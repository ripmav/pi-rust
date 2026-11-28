//! The user extension's process runtime (WP-1205, [ADR 0001](../docs/adr/0001-rust-extensions-as-bridge-subprocesses.md)):
//! turns an [`Extension`] into a bridge-v6-speaking process on stdio —
//! the same framing, `init`/`ready` handshake, message catalog and
//! response/event semantics as the Node sidecar
//! (docs/bridge-protocol.md), so from the core's point of view a Rust
//! extension is indistinguishable from a TS extension.
//!
//! An extension binary's `main` is:
//!
//! ```ignore
//! fn main() {
//!     pi_ext::process::run(MyExtension);
//! }
//! ```
//!
//! 1. **Handshake** — wait for `init`, reject a non-six
//!    `protocolVersion` (init error response + non-zero exit), run
//!    [`Extension::init`] (a panic → init error + exit 1), answer
//!    `apiVersion: 0`, emit `ready` with the catalog collected during
//!    `init` (tools/commands/shortcuts — resolved against
//!    `init.keybindings`, upstream `getShortcuts`: a reserved built-in
//!    on the key is blocked, a non-reserved one shadowed —
//!    flags/providers, the events the extension declares, the v5 initial
//!    active tool set), forward
//!    load-time MCP registrations to the core as `mcp_server_register`
//!    requests (sidecar order: the forward lines go out **before** the
//!    init response and `ready`; a rejected forward → a typed
//!    `extension_error`) and only then answer `apiVersion: 0` +
//!    `ready`.
//! 2. **Dispatch loop** — every command line is handled by its own
//!    task (bidirectional liveness: a handler may block on a dialog or
//!    an extension→core request while `extension_ui_response` /
//!    `pane_key` / responses keep flowing, no head-of-line blocking):
//!    fire-and-forget events (unknown/unsubscribed → logged, never
//!    fatal), decision commands (protocol defaults when nothing is
//!    subscribed, the combined result otherwise), `command` /
//!    `shortcut` / `extension_tool_call` / `provider_stream` (ack + one
//!    `provider_stream_event` line per upstream event, terminal
//!    `done`/`error` ends the stream) and `session_shutdown` (handler
//!    runs, then the process exits 0 — also when the handler panics,
//!    after the `extension_error`).
//! 3. **UI round-trip** — the [`ExtensionUI`] dialogs emit
//!    `extension_ui_request` and wait for `extension_ui_response`;
//!    the `init`-negotiated `timeoutMs` is the fallback timeout and a
//!    timeout resolves to the upstream default (`confirm` → `false`,
//!    the rest → `None`). Unsupported methods (not in the advertised
//!    `ui` list) resolve to that default without a request, the
//!    fire-and-forget methods always emit. Custom panes use
//!    `pane_open` / `pane_update` / `pane_close` + `pane_key`: the
//!    pane opens on the component's first host call (sidecar order —
//!    a failed factory emits no `pane_open`), ends with `done` (a
//!    lingering factory task is aborted, not awaited) or the
//!    capability timeout (which also closes an opened pane).
//!
//! **Panic safety**: extension code runs in a child task, so a handler
//! panic is captured and never kills the process. A panicking decision
//! handler emits `extension_error` and the decision resolves to the
//! protocol default (e.g. `tool_call` → no block, input unchanged); a
//! panicking command/tool/shortcut gets a clean error response; a
//! provider stream ends with a terminal `error` event; a panicking
//! pane factory emits `extension_error` (+ `pane_close` if the pane
//! had already opened) and resolves to the upstream default right
//! away. stdout carries complete lines only: a single writer task
//! serializes every write, and all logs go to stderr. Every exit path
//! (stdin EOF, handshake failure, `session_shutdown`) drains the
//! writer's queue first — no queued line is lost.

use crate::pi::{
    ExecResult, Extension, ExtensionContext, ExtensionTransport, ExtensionUI, NotifyKind, Pane,
    PaneDescriptor, PaneHost, Pi, SendUserMessageOptions, Shortcut, ToolContext, UiDialogOptions,
};
use crate::wire::{
    AgentEndEvent, AgentMessage, AgentStartEvent, AgentToolResult, AssistantMessage,
    AssistantMessageEvent, BeforeAgentStartEvent, ContentBlock, ContextEvent,
    ContextWithSystemEvent, ExtensionError, ExtensionFlag, InputEvent, McpServerConfig,
    McpServersChangeEvent, MessageContent, MessageEndEvent, MessageStartEvent, MessageUpdateEvent,
    ModelRef, ModelSelectEvent, PaneBlocked, PaneKeyInput, Provider, ProviderStreamOptions, Ready,
    ReadyCommand, ReadyExtension, ReadyShortcut, ReadyTool, SessionBeforeCompactEvent,
    SessionCompactEvent, SessionCompactFailedEvent, SessionInfoChanged, SessionShutdownEvent,
    SessionStartEvent, SidecarEvent, ThinkingLevelSelectEvent, ToolCallEvent, ToolResultEvent,
    TurnEndEvent, TurnStartEvent, UiRequest, UserBashEvent,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Run `ext` as a bridge-v6 process on stdio. Never returns: the
/// process exits on stdin EOF (0), `session_shutdown` (0) or a
/// handshake failure (1).
pub fn run<E>(ext: E)
where
    E: Extension + 'static,
{
    install_panic_hook();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("building the tokio runtime");
    rt.block_on(async move {
        let path = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let ctx = ExtensionContext {
            mode: "rpc".into(),
            cwd,
            ..Default::default()
        };
        let b = Bridge::start(tokio::io::stdout(), ctx, path);
        // ONE stdin reader for the whole lifetime (sidecar pump): the
        // handshake and the dispatch loop share the same buffer, so a
        // line that arrives in the same pipe chunk as `init` (after it)
        // is not consumed by the handshake's read and then lost.
        let mut reader = BufReader::new(tokio::io::stdin());
        let (ext, pi) = handshake(b.clone(), ext, &mut reader).await;
        dispatch_loop(b, ext, pi, &mut reader).await;
    });
    std::process::exit(0); // unreachable: the loop always exits the process
}

// =====================================================================
// shared runtime state
// =====================================================================

/// One item for the stdout writer: a complete protocol line or a
/// flush-and-exit signal.
enum OutLine {
    Line(String),
    Shutdown(i32),
}

/// An open custom pane: key waiters (FIFO, one per pending
/// `Pane::next_key`) + the `done` waiter + the lazy `pane_open` /
/// `pane_close` state.
struct PaneState {
    waiters: Mutex<VecDeque<oneshot::Sender<PaneKeyInput>>>,
    done: Mutex<Option<oneshot::Sender<Value>>>,
    /// The lazy `pane_open` has gone out (the component's first host
    /// call fired it; a factory that fails before that opens nothing).
    opened: AtomicBool,
    /// The `pane_close` has gone out (idempotent close).
    closed: AtomicBool,
    /// The `custom`-negotiated `blocked` (carried by the lazy `pane_open`).
    blocked: Option<PaneBlocked>,
    /// The `done(result)` value (stored before the done waiter resolves).
    result: Mutex<Option<Value>>,
}

/// Shared runtime state: the single stdout writer's channel, the
/// extension→core request / dialog / pane waiters, the negotiated
/// capabilities, the subscribed events and the live ctx view.
struct Bridge {
    out: mpsc::UnboundedSender<OutLine>,
    /// extension→core requests awaiting a `response`: id → waiter.
    pending: Mutex<std::collections::HashMap<String, oneshot::Sender<Value>>>,
    /// Open dialogs awaiting `extension_ui_response`: id → waiter.
    dialogs: Mutex<std::collections::HashMap<String, oneshot::Sender<Value>>>,
    /// Open custom panes: id → state.
    panes: Mutex<std::collections::HashMap<String, PaneState>>,
    /// The `init`-advertised `ui` methods (set at the handshake).
    ui_methods: Mutex<Vec<String>>,
    /// The `init`-negotiated `timeoutMs` (set at the handshake).
    timeout_ms: Mutex<u64>,
    /// `ready.events` — the event names the extension declared.
    subscribed: Mutex<Vec<String>>,
    /// The live context view (model / thinking level move on F&F events).
    ctx: Mutex<ExtensionContext>,
    /// `session_shutdown` in flight: queued commands are dropped.
    shutting_down: AtomicBool,
    /// This extension's binary path (`ready.extensions[].path`,
    /// `extension_error.extensionPath`).
    path: String,
}

/// The last panic message (single-threaded runtime: the hook runs in
/// the panicking task; the dispatch task reads it after the join).
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);
/// Correlation ids for extension→core requests and dialogs (opaque
/// tokens; the protocol only requires uniqueness per process).
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn next_id(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
}

/// Remember the panic message (the process hook) and keep the default
/// stderr output.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "panic".into());
        *LAST_PANIC.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
        prev(info);
    }));
}

fn take_last_panic() -> String {
    LAST_PANIC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_else(|| "extension handler panicked".into())
}

impl Bridge {
    fn start(
        stdout: impl AsyncWrite + Unpin + Send + 'static,
        ctx: ExtensionContext,
        path: String,
    ) -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        // The single stdout writer: one write_all per complete line, so
        // a panic can never leave a half-line on the pipe.
        tokio::spawn(async move {
            let mut out = stdout;
            while let Some(item) = rx.recv().await {
                match item {
                    OutLine::Line(line) => {
                        if out.write_all(line.as_bytes()).await.is_err()
                            || out.flush().await.is_err()
                        {
                            eprintln!("[pi-ext] stdout write failed; exiting");
                            std::process::exit(1);
                        }
                    }
                    OutLine::Shutdown(code) => {
                        let _ = out.flush().await;
                        std::process::exit(code);
                    }
                }
            }
        });
        Arc::new(Self {
            out: tx,
            pending: Mutex::new(std::collections::HashMap::new()),
            dialogs: Mutex::new(std::collections::HashMap::new()),
            panes: Mutex::new(std::collections::HashMap::new()),
            ui_methods: Mutex::new(Vec::new()),
            timeout_ms: Mutex::new(30000),
            subscribed: Mutex::new(Vec::new()),
            ctx: Mutex::new(ctx),
            shutting_down: AtomicBool::new(false),
            path,
        })
    }

    fn emit(&self, v: &Value) {
        let Ok(mut line) = serde_json::to_string(v) else {
            return; // a Value always serializes; unreachable
        };
        line.push('\n');
        let _ = self.out.send(OutLine::Line(line));
    }

    fn emit_typed<T: Serialize>(&self, v: &T) {
        let Ok(v) = serde_json::to_value(v) else {
            return;
        };
        self.emit(&v);
    }

    fn emit_stream_event(&self, stream_id: &str, event: &AssistantMessageEvent) {
        self.emit(&json!({
            "type": "provider_stream_event",
            "streamId": stream_id,
            "event": event,
        }));
    }

    fn emit_error(&self, event: &str, msg: &str) {
        self.emit_typed(&SidecarEvent::ExtensionError(ExtensionError {
            extension_path: self.path.clone(),
            event: event.to_string(),
            error: msg.to_string(),
            stack: None,
        }));
    }

    fn set_capabilities(&self, ui: Vec<String>, timeout_ms: u64) {
        *self.ui_methods.lock().unwrap() = ui;
        *self.timeout_ms.lock().unwrap() = timeout_ms;
    }

    fn ui_supported(&self, method: &str) -> bool {
        self.ui_methods.lock().unwrap().iter().any(|m| m == method)
    }

    fn subscribed(&self, name: &str) -> bool {
        self.subscribed.lock().unwrap().iter().any(|n| n == name)
    }

    fn ctx_snapshot(&self) -> ExtensionContext {
        self.ctx.lock().unwrap().clone()
    }

    /// Emit `extension_ui_request` and wait for `extension_ui_response`
    /// (or the dialog timeout → `None`, the upstream default).
    async fn send_dialog(&self, req: &UiRequest) -> Option<Value> {
        let v = serde_json::to_value(req).expect("UiRequest serializes");
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let timeout = v
            .get("timeout")
            .and_then(Value::as_u64)
            .unwrap_or(*self.timeout_ms.lock().unwrap());
        let (tx, rx) = oneshot::channel::<Value>();
        self.dialogs.lock().unwrap().insert(id.clone(), tx);
        self.emit_typed(&SidecarEvent::ExtensionUiRequest(req.clone()));
        let answer = tokio::time::timeout(std::time::Duration::from_millis(timeout), rx)
            .await
            .ok()
            .and_then(Result::ok);
        // The dialog is over: answered (`on_ui_response` already took
        // the entry) or timed out — remove ours so a late
        // `extension_ui_response` reads "unknown dialog" instead of
        // sending into a dropped waiter (request() removes on timeout
        // the same way).
        self.dialogs.lock().unwrap().remove(&id);
        answer
    }

    /// Emit an extension→core req/resp line (`{ id, type, …payload }`)
    /// and return the id + waiter (the caller owns the wait).
    fn request_emit(
        &self,
        rtype: &'static str,
        fields: Value,
    ) -> (String, oneshot::Receiver<Value>) {
        let id = next_id("req");
        let mut m = fields.as_object().cloned().unwrap_or_default();
        m.insert("id".into(), json!(id));
        m.insert("type".into(), json!(rtype));
        let (tx, rx) = oneshot::channel::<Value>();
        self.pending.lock().unwrap().insert(id.clone(), tx);
        self.emit(&Value::Object(m));
        (id, rx)
    }

    /// Wait for the `response` to a line emitted with [`request_emit`]
    /// (or the capability timeout → a typed failure).
    async fn wait_response(
        &self,
        rtype: &'static str,
        id: &str,
        rx: oneshot::Receiver<Value>,
    ) -> Value {
        let timeout = std::time::Duration::from_millis(*self.timeout_ms.lock().unwrap());
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => resp,
            _ => {
                self.pending.lock().unwrap().remove(id);
                json!({
                    "id": id,
                    "type": "response",
                    "command": rtype,
                    "success": false,
                    "error": "request to core timed out",
                })
            }
        }
    }

    /// An extension→core req/resp line (`{ id, type, …payload }`): emit,
    /// wait for the `response` (or the capability timeout → a typed
    /// failure).
    async fn request(&self, rtype: &'static str, fields: Value) -> Value {
        let (id, rx) = self.request_emit(rtype, fields);
        self.wait_response(rtype, &id, rx).await
    }

    /// A `response` line for one of our requests (the main loop calls
    /// this); unknown/already-timed-out ids are logged and ignored.
    fn on_response(&self, v: &Value) {
        let Some(id) = v.get("id").and_then(Value::as_str) else {
            return;
        };
        match self.pending.lock().unwrap().remove(id) {
            Some(waiter) => {
                let _ = waiter.send(v.clone());
            }
            None => eprintln!("[pi-ext] response for unknown or timed-out request {id} ignored"),
        }
    }

    /// An `extension_ui_response` line (the main loop calls this).
    fn on_ui_response(&self, v: &Value) {
        let Some(id) = v.get("id").and_then(Value::as_str) else {
            return;
        };
        match self.dialogs.lock().unwrap().remove(id) {
            Some(waiter) => {
                let _ = waiter.send(v.clone());
            }
            None => eprintln!("[pi-ext] extension_ui_response for unknown dialog {id} ignored"),
        }
    }

    /// A `pane_key` line for an open custom pane (the main loop calls
    /// this).
    fn on_pane_key(&self, v: &Value) {
        let Some(id) = v.get("id").and_then(Value::as_str) else {
            return;
        };
        let Ok(key) =
            serde_json::from_value::<PaneKeyInput>(v.get("key").cloned().unwrap_or(Value::Null))
        else {
            eprintln!("[pi-ext] pane_key with a malformed key ignored");
            return;
        };
        let panes = self.panes.lock().unwrap();
        let waiter = match panes.get(id) {
            Some(st) => st.waiters.lock().unwrap().pop_front(),
            None => {
                eprintln!("[pi-ext] pane_key for unknown pane {id} ignored");
                return;
            }
        };
        match waiter {
            Some(waiter) => {
                let _ = waiter.send(key);
            }
            None => eprintln!("[pi-ext] pane_key for pane {id} without a waiter dropped"),
        }
    }

    /// The lazy `pane_open` (sidecar order: `pane_open` goes out only
    /// for a live component — the sidecar emits it after a successful
    /// factory, before the first update; the Rust equivalent moment is
    /// the component's first host call). Returns `false` when the pane
    /// is already gone (ended / timed out): the caller no-ops.
    fn open_pane(&self, id: &str) -> bool {
        let blocked = {
            let panes = self.panes.lock().unwrap();
            let Some(st) = panes.get(id) else {
                return false;
            };
            if st
                .opened
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return true; // already open
            }
            st.blocked.clone()
        };
        self.emit_typed(&SidecarEvent::PaneOpen {
            id: id.to_string(),
            blocked,
        });
        true
    }

    /// The single `pane_close` for a pane (idempotent across the done /
    /// timeout / panic / ended paths): only for a pane that opened
    /// (the sidecar closes only panes it opened) and only once.
    fn emit_pane_close_once(&self, id: &str) {
        let panes = self.panes.lock().unwrap();
        let Some(st) = panes.get(id) else {
            return;
        };
        if !st.opened.load(Ordering::SeqCst)
            || st
                .closed
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            return;
        }
        self.emit_typed(&SidecarEvent::PaneClose { id: id.to_string() });
    }

    /// The `done(result)` value if the component called `done` (it is
    /// stored before the done waiter resolves, so it is readable even
    /// when the factory task finishes at the same moment).
    fn pane_result(&self, id: &str) -> Option<Value> {
        self.panes
            .lock()
            .unwrap()
            .get(id)
            .and_then(|st| st.result.lock().unwrap().clone())
    }

    /// A panic in the extension code (the dispatch task calls this):
    /// the protocol default per command kind, never a crash.
    async fn on_panic(&self, policy: &Policy, id: Option<&str>, rtype: &str) {
        let msg = take_last_panic();
        match policy {
            Policy::FireAndForget(event) => {
                self.emit_error(event, &msg);
                // A `session_shutdown` handler that ran (a panic is the
                // case that reached here) still ends the process: the
                // core waits for the child's exit. Error first, exit 0.
                if event == "session_shutdown" {
                    self.exit_flush(0).await;
                }
            }
            Policy::Decision(event, default_data) => {
                self.emit_error(event, &msg);
                self.emit(&response_line(
                    id,
                    rtype,
                    true,
                    Some(default_data.clone()),
                    None,
                ));
            }
            Policy::ReqResp(command) => {
                eprintln!("[pi-ext] command {command} failed: {msg}");
                self.emit(&response_line(id, command, false, None, Some(msg)));
            }
            Policy::Stream { stream_id, model } => {
                self.emit_stream_event(
                    stream_id,
                    &provider_error_event(model, &format!("provider stream panicked: {msg}")),
                );
            }
        }
    }

    /// Forward a load-time MCP registration to the core (v6
    /// `mcp_server_register`); a rejected forward is a typed
    /// `extension_error`, never a crash. The forward line is emitted
    /// synchronously (sidecar order: before the init response + ready);
    /// only the response wait and the typed failure run in a task.
    fn flush_mcp_server(b: &Arc<Bridge>, name: &str, config: &McpServerConfig) {
        let b = Arc::clone(b);
        let name = name.to_string();
        let config = config.clone();
        let (id, rx) = b.request_emit(
            "mcp_server_register",
            json!({ "name": name, "config": config, "extensionPath": b.path }),
        );
        tokio::spawn(async move {
            let resp = b.wait_response("mcp_server_register", &id, rx).await;
            if resp.get("success") != Some(&Value::Bool(true)) {
                let err = resp
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                b.emit_error(
                    "mcp_server_register",
                    &format!("registerMcpServer(\"{name}\") was not hosted by the core: {err}"),
                );
            }
        });
    }

    /// Flush the queued protocol lines and exit with `code` (the writer
    /// does the final flush + exit; this future never returns).
    async fn exit_flush(&self, code: i32) -> ! {
        let _ = self.out.send(OutLine::Shutdown(code));
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

/// What a panic in a command's handler must produce (protocol error
/// model, per command kind).
enum Policy {
    /// Fire-and-forget event: `extension_error` only, no response.
    FireAndForget(String),
    /// Decision: `extension_error` + success with the protocol default
    /// `data`.
    Decision(String, Value),
    /// `command`/`shortcut`/`extension_tool_call`: clean error response.
    ReqResp(String),
    /// `provider_stream`: a terminal `error` event for the stream.
    Stream { stream_id: String, model: ModelRef },
}

fn classify(rtype: &str, v: &Value) -> Policy {
    match rtype {
        "session_start"
        | "agent_start"
        | "agent_end"
        | "turn_start"
        | "turn_end"
        | "message_start"
        | "message_update"
        | "message_end"
        | "set_model"
        | "set_thinking_level"
        | "session_compact"
        | "session_compact_failed"
        | "session_shutdown"
        | "mcp_servers_change" => Policy::FireAndForget(rtype.to_string()),
        "before_agent_start" => Policy::Decision(rtype.to_string(), json!({})),
        "tool_call" => Policy::Decision(
            rtype.to_string(),
            json!({ "result": null, "input": v["event"]["input"] }),
        ),
        "tool_result" | "user_bash" | "session_before_compact" | "context" => {
            Policy::Decision(rtype.to_string(), Value::Null)
        }
        "context_with_system" => {
            Policy::Decision(rtype.to_string(), v["event"]["messages"].clone())
        }
        "input" => Policy::Decision(rtype.to_string(), json!({ "action": "continue" })),
        "command" | "shortcut" | "extension_tool_call" => Policy::ReqResp(rtype.to_string()),
        "provider_stream" => {
            let stream_id = v
                .get("streamId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "stream".to_string());
            match v
                .get("model")
                .cloned()
                .and_then(|m| serde_json::from_value::<ModelRef>(m).ok())
            {
                Some(model) => Policy::Stream { stream_id, model },
                None => Policy::ReqResp(rtype.to_string()),
            }
        }
        _ => Policy::ReqResp(rtype.to_string()),
    }
}

// =====================================================================
// handshake
// =====================================================================

/// `{ id, type: "response", command, success, data? | error? }`.
fn response_line(
    id: Option<&str>,
    command: &str,
    success: bool,
    data: Option<Value>,
    error: Option<String>,
) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), id.map(Value::from).unwrap_or(Value::Null));
    m.insert("type".into(), json!("response"));
    m.insert("command".into(), json!(command));
    m.insert("success".into(), json!(success));
    if success {
        if let Some(d) = data {
            m.insert("data".into(), d);
        }
    } else {
        m.insert("error".into(), json!(error.unwrap_or_default()));
    }
    Value::Object(m)
}

fn parse_error_line(msg: &str) -> Value {
    response_line(None, "parse", false, None, Some(msg.to_string()))
}

/// v0 hardening (sidecar `MAX_LINE`): a single NDJSON command line
/// never exceeds 1 MB — reject it (parse-error model) instead of letting
/// JSON.parse eat the memory.
const MAX_LINE: usize = 1024 * 1024;

/// One capped stdin line (the sidecar pump's line rules).
enum CappedLine {
    /// A complete line (≤ `MAX_LINE`).
    Line(String),
    /// EOF (a non-empty tail was already returned as a `Line`).
    Eof,
    /// A complete line exceeded `MAX_LINE` (its byte length, for the
    /// error); the whole line is discarded.
    TooLong(usize),
    /// An unterminated buffer exceeded `MAX_LINE`; the buffer was
    /// reset, so the next line starts fresh.
    Unterminated,
}

/// The next line of the stdin pump with the `MAX_LINE` cap:
/// - a line past the cap → `TooLong` (reported, line discarded);
/// - an unterminated buffer past the cap → `Unterminated` (reported,
///   buffer reset — it would swallow every later line);
/// - EOF → `Eof`, after flushing a non-empty tail as a final record
///   (upstream jsonl semantics).
async fn next_capped_line(reader: &mut BufReader<tokio::io::Stdin>) -> std::io::Result<CappedLine> {
    let mut line = String::new();
    loop {
        let (consume, next) = {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                // EOF: flush a non-empty tail as a final record.
                (
                    0usize,
                    Some(if line.is_empty() {
                        CappedLine::Eof
                    } else {
                        CappedLine::Line(std::mem::take(&mut line))
                    }),
                )
            } else if let Some(i) = buf.iter().position(|&b| b == b'\n') {
                if line.len() + i > MAX_LINE {
                    (i + 1, Some(CappedLine::TooLong(line.len() + i)))
                } else {
                    line.push_str(&String::from_utf8_lossy(&buf[..i]));
                    (i + 1, Some(CappedLine::Line(std::mem::take(&mut line))))
                }
            } else if line.len() + buf.len() > MAX_LINE {
                (buf.len(), Some(CappedLine::Unterminated))
            } else {
                line.push_str(&String::from_utf8_lossy(buf));
                (buf.len(), None) // keep accumulating
            }
        };
        reader.consume(consume);
        if let Some(next) = next {
            return Ok(next);
        }
    }
}

/// The sidecar's typed parse error for a capped line.
fn line_cap_error(c: &CappedLine) -> String {
    match c {
        CappedLine::TooLong(len) => {
            format!("line too long ({len} bytes, max {MAX_LINE})")
        }
        CappedLine::Unterminated => {
            format!("line too long (unterminated, max {MAX_LINE} bytes)")
        }
        _ => unreachable!("only the capped variants reach here"),
    }
}

/// Wait for `init`, validate it, run `Extension::init`, answer with
/// `apiVersion` and emit `ready`. Ends the process (1) on a handshake
/// failure.
async fn handshake<E: Extension>(
    b: Arc<Bridge>,
    mut ext: E,
    reader: &mut BufReader<tokio::io::Stdin>,
) -> (Arc<E>, Arc<Pi>) {
    // 1. wait for the init command (tolerating everything else the
    //    protocol allows before init).
    let init_v = loop {
        let line = match next_capped_line(reader).await {
            Ok(CappedLine::Line(l)) => l,
            Ok(CappedLine::Eof) => {
                eprintln!("[pi-ext] stdin closed before init");
                b.exit_flush(1).await
            }
            Ok(e) => {
                b.emit(&parse_error_line(&line_cap_error(&e)));
                continue;
            }
            Err(e) => {
                eprintln!("[pi-ext] reading stdin: {e}");
                b.exit_flush(1).await
            }
        };
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            b.emit(&parse_error_line("Failed to parse command: not valid JSON"));
            continue;
        };
        let t = v.get("type").and_then(Value::as_str).unwrap_or("unknown");
        match t {
            "init" => break v,
            // No requests/dialogs/panes exist yet: nothing to resolve.
            "response" | "extension_ui_response" | "pane_key" => {
                eprintln!("[pi-ext] {t} before init ignored");
            }
            _ => {
                if let Some(id) = v.get("id").and_then(Value::as_str) {
                    b.emit(&response_line(
                        Some(id),
                        t,
                        false,
                        None,
                        Some("extension not initialized; send init first".to_string()),
                    ));
                } else {
                    eprintln!("[pi-ext] unknown or unsubscribed event \"{t}\" ignored");
                }
            }
        }
    };

    let id = init_v.get("id").and_then(Value::as_str).map(str::to_string);
    // 2. validate protocolVersion (sidecar rule: reject non-six, exit
    //    non-zero).
    let version = init_v.get("protocolVersion").and_then(Value::as_u64);
    if version != Some(6) {
        let shown = version
            .map(|v| v.to_string())
            .unwrap_or_else(|| "absent".into());
        b.emit(&response_line(
            id.as_deref(),
            "init",
            false,
            None,
            Some(format!(
                "Unsupported protocolVersion: {shown} (this extension speaks 6)"
            )),
        ));
        b.exit_flush(1).await;
    }
    // 3. capabilities (sidecar defaults: `ui: []`, `timeoutMs: 30000`).
    let caps = init_v.get("capabilities");
    let ui: Vec<String> = caps
        .and_then(|c| c.get("ui"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let timeout_ms = caps
        .and_then(|c| c.get("timeoutMs"))
        .and_then(Value::as_u64)
        .unwrap_or(30000);
    b.set_capabilities(ui.clone(), timeout_ms);
    b.ctx.lock().unwrap().has_ui = !ui.is_empty();

    // 3b. The core's resolved keybinding table (`init.keybindings`,
    //     action → keys; v4): the registered shortcuts resolve against
    //     it for `ready` (upstream `runner.getShortcuts`).
    let keybindings = parse_keybindings(init_v.get("keybindings"));

    // 4. run the extension's init (a panic = handshake failure).
    let transport = BridgeTransport(b.clone());
    let mut pi = Pi::new(b.ctx_snapshot(), Box::new(transport));
    // The handler's ctx view shares the transport's UI surface (Pi::new
    // wires pi.ctx.ui from the transport; the live snapshot defaults to
    // the headless NullUi).
    b.ctx.lock().unwrap().ui = pi.ctx.ui.clone();
    let init_ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ext.init(&mut pi);
    }));
    if init_ok.is_err() {
        let msg = take_last_panic();
        eprintln!("[pi-ext] extension init panicked: {msg}");
        b.emit(&response_line(
            id.as_deref(),
            "init",
            false,
            None,
            Some(format!("extension init failed: {msg}")),
        ));
        b.exit_flush(1).await;
    }

    // 5. the catalog (sidecar order: the load-time MCP forward lines go
    //    out before the init response and ready).
    let events: Vec<String> = ext
        .events()
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let path = b.path.clone();
    for server in pi.mcp_servers() {
        Bridge::flush_mcp_server(&b, &server.name, &server.config);
    }
    let ready = ready_payload(&pi, &path, &events, &keybindings);
    b.emit(&response_line(
        id.as_deref(),
        "init",
        true,
        Some(json!({ "apiVersion": 0 })),
        None,
    ));
    b.emit_typed(&SidecarEvent::Ready(ready.clone()));
    b.subscribed.lock().unwrap().clone_from(&events);
    eprintln!(
        "[pi-ext] ready: {} tools, {} commands, {} providers (events: {})",
        ready.tools.len(),
        ready.commands.len(),
        ready.providers.len(),
        events.join(", ")
    );

    (Arc::new(ext), Arc::new(pi))
}

/// The 18 reserved editor-global keybinding ids (upstream
/// `runner.ts:98`): an extension shortcut on one of their keys is
/// blocked (skipped), not shadowed.
const RESERVED_KEYBINDINGS: &[&str] = &[
    "app.interrupt",
    "app.clear",
    "app.exit",
    "app.suspend",
    "app.thinking.cycle",
    "app.model.cycleForward",
    "app.model.cycleBackward",
    "app.model.select",
    "app.tools.expand",
    "app.thinking.toggle",
    "app.editor.external",
    "app.message.copy",
    "app.message.followUp",
    "tui.input.submit",
    "tui.select.confirm",
    "tui.select.cancel",
    "tui.input.copy",
    "tui.editor.deleteToLineEnd",
];

/// `init.keybindings` → the resolved action → keys table (upstream
/// `buildBuiltinKeybindings` accepts a single key or a list per action).
fn parse_keybindings(v: Option<&Value>) -> BTreeMap<String, Vec<String>> {
    let Some(obj) = v.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (action, keys) in obj {
        let keys = match keys {
            Value::String(k) => vec![k.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect(),
            _ => continue,
        };
        out.insert(action.clone(), keys);
    }
    out
}

/// Upstream `getShortcuts` resolution of the registered shortcuts
/// against the core's keybinding table: a **reserved** built-in on the
/// key blocks the shortcut (skipped + conflict diagnostic); a
/// **non-reserved** built-in is shadowed (the extension wins +
/// diagnostic); when two actions share a key the reserved one wins the
/// merge; duplicates on the same (lowercased) key are deduped — the
/// later registration wins + conflict diagnostic (upstream Map
/// semantics). Returns the `ready.shortcuts` set +
/// `ready.shortcutDiagnostics`.
fn resolve_shortcuts(
    shortcuts: &[Shortcut],
    keybindings: &BTreeMap<String, Vec<String>>,
    path: &str,
) -> (Vec<ReadyShortcut>, Vec<String>) {
    // Lowercased key → (action, reserved) of the built-in on that key.
    let mut builtins: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for (action, keys) in keybindings {
        let reserved = RESERVED_KEYBINDINGS.contains(&action.as_str());
        for key in keys {
            let key = key.to_lowercase();
            // A reserved binding already owns this key: it stays.
            if builtins.get(&key).is_some_and(|(_, r)| *r) && !reserved {
                continue;
            }
            builtins.insert(key, (action.clone(), reserved));
        }
    }
    let mut resolved = Vec::new();
    let mut diagnostics = Vec::new();
    // Upstream `extensionShortcuts` is a Map keyed by the lowercased
    // key: a duplicate is replaced in place — the later registration
    // wins (Map.set keeps the original position).
    let mut by_key: BTreeMap<String, usize> = BTreeMap::new();
    for s in shortcuts {
        let key = s.key.to_lowercase();
        let blocked = builtins.get(&key).is_some_and(|(_, reserved)| *reserved);
        if blocked {
            diagnostics.push(format!(
                "Extension shortcut '{}' from {} conflicts with built-in shortcut. Skipping.",
                s.key, path
            ));
            continue;
        }
        if let Some((action, false)) = builtins.get(&key) {
            diagnostics.push(format!(
                "Extension shortcut conflict: '{}' is built-in shortcut for {} and {}. Using {}.",
                s.key, action, path, path
            ));
        }
        let entry = ReadyShortcut {
            key: s.key.clone(),
            description: s.description.clone(),
            extension_path: path.to_string(),
        };
        match by_key.get(&key) {
            Some(&pos) => {
                diagnostics.push(format!(
                    "Extension shortcut conflict: '{}' registered by both {} and {}. Using {}.",
                    s.key, path, path, path
                ));
                resolved[pos] = entry; // last wins (position kept)
            }
            None => {
                by_key.insert(key, resolved.len());
                resolved.push(entry);
            }
        }
    }
    (resolved, diagnostics)
}

/// The `ready` registry snapshot from the stored registrations.
/// (`keybindings`: the resolved table for the shortcut resolution.)
fn ready_payload(
    pi: &Pi,
    path: &str,
    events: &[String],
    keybindings: &BTreeMap<String, Vec<String>>,
) -> Ready {
    let tools: Vec<ReadyTool> = pi
        .tools()
        .iter()
        .map(|t| ReadyTool {
            name: t.name.clone(),
            label: t.label.clone(),
            description: t.description.clone(),
            prompt_snippet: t.prompt_snippet.clone(),
            parameters: t.parameters.clone(),
            exposure: t.exposure.clone(),
            default_active: t.default_active,
        })
        .collect();
    // v5 active tool set: `direct`/`model-only` tools activate unless
    // `defaultActive: false`.
    let active_tools = pi
        .tools()
        .iter()
        .filter(|t| {
            let exposure = t.exposure.as_deref().unwrap_or("direct");
            matches!(exposure, "direct" | "model-only") && t.default_active != Some(false)
        })
        .map(|t| t.name.clone())
        .collect();
    // Upstream `getShortcuts` resolution (reserved built-in on the key
    // blocks the shortcut; a non-reserved one is shadowed).
    let shortcuts = resolve_shortcuts(pi.shortcuts(), keybindings, path);
    Ready {
        api_version: 0,
        extensions: vec![ReadyExtension {
            path: path.to_string(),
            hidden: None,
        }],
        errors: Vec::new(),
        tools,
        commands: pi
            .commands()
            .iter()
            .map(|c| ReadyCommand {
                name: c.name.clone(),
                description: c.description.clone(),
            })
            .collect(),
        shortcuts: Some(shortcuts.0),
        shortcut_diagnostics: Some(shortcuts.1),
        flags: pi
            .flags()
            .iter()
            .map(|f| ExtensionFlag {
                name: f.name.clone(),
                description: f.description.clone(),
                kind: f.kind.as_str().to_string(),
                default: f.default.clone(),
                extension_path: path.to_string(),
            })
            .collect(),
        providers: pi
            .providers()
            .iter()
            .map(|p| Provider {
                name: p.name.clone(),
                models: p
                    .config
                    .models
                    .iter()
                    .map(|m| ModelRef {
                        provider: p.name.clone(),
                        id: m.id.clone(),
                        api: m.api.clone(),
                        name: m.name.clone(),
                    })
                    .collect(),
            })
            .collect(),
        events: events.to_vec(),
        active_tools: Some(active_tools),
    }
}

// =====================================================================
// dispatch loop
// =====================================================================

/// The main loop: read stdin, dispatch every line to its own task
/// (bidirectional liveness). Ends the process (0) on stdin EOF.
async fn dispatch_loop<E: Extension + 'static>(
    b: Arc<Bridge>,
    ext: Arc<E>,
    pi: Arc<Pi>,
    reader: &mut BufReader<tokio::io::Stdin>,
) {
    loop {
        let line = match next_capped_line(reader).await {
            Ok(CappedLine::Line(l)) => l,
            Ok(CappedLine::Eof) => {
                eprintln!("[pi-ext] stdin closed; shutting down");
                // The async shutdown path (sidecar `shutdown()`): the
                // writer drains its queue, flushes, and only then exits
                // — a raw `process::exit` here would drop queued lines
                // and can die mid-line.
                b.exit_flush(0).await
            }
            Ok(e) => {
                b.emit(&parse_error_line(&line_cap_error(&e)));
                continue;
            }
            Err(e) => {
                eprintln!("[pi-ext] reading stdin: {e}");
                b.exit_flush(0).await
            }
        };
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            b.emit(&parse_error_line("Failed to parse command: not valid JSON"));
            continue;
        };
        if b.shutting_down.load(Ordering::SeqCst) {
            continue; // shutdown in flight: drop queued commands
        }
        dispatch_line(b.clone(), ext.clone(), pi.clone(), v);
    }
}

fn dispatch_line<E: Extension + 'static>(b: Arc<Bridge>, ext: Arc<E>, pi: Arc<Pi>, v: Value) {
    let rtype: String = v
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let id = v.get("id").and_then(Value::as_str).map(str::to_string);
    match rtype.as_str() {
        // Lines that resolve waiters, not handlers (no user code).
        "response" => {
            b.on_response(&v);
            return;
        }
        "extension_ui_response" => {
            b.on_ui_response(&v);
            return;
        }
        "pane_key" => {
            b.on_pane_key(&v);
            return;
        }
        "init" => {
            eprintln!("[pi-ext] second init ignored");
            if let Some(id) = &id {
                b.emit(&response_line(
                    Some(id),
                    "init",
                    false,
                    None,
                    Some("already initialized".into()),
                ));
            }
            return;
        }
        _ => {}
    }
    if !is_known_type(rtype.as_str()) {
        // A protocol error, not a crash (rpc-mode.ts:734-737).
        b.emit(&response_line(
            id.as_deref(),
            rtype.as_str(),
            false,
            None,
            Some(format!("Unknown command: {rtype}")),
        ));
        return;
    }
    // Everything else: a command or an event for a handler (user code
    // runs in its own task, with a child task so a handler panic is
    // captured — process safety).
    let policy = classify(rtype.as_str(), &v);
    tokio::spawn(async move {
        // The inner task owns its clones (the outer keeps b to emit the
        // result or the panic's protocol response).
        let (b3, e3, p3) = (b.clone(), ext.clone(), pi.clone());
        let inner = tokio::spawn(async move { run_command::<E>(&b3, &e3, &p3, v).await });
        match inner.await {
            Ok(out) => {
                if let Some(line) = out {
                    b.emit(&line);
                }
            }
            Err(je) if je.is_panic() => b.on_panic(&policy, id.as_deref(), rtype.as_str()).await,
            Err(_) => {
                // runtime gone; nothing left to do
            }
        }
    });
}

/// The command/event types the runtime dispatches (the v6 catalog's
/// core→extension side); anything else is a protocol error.
fn is_known_type(t: &str) -> bool {
    matches!(
        t,
        "session_start"
            | "agent_start"
            | "agent_end"
            | "turn_start"
            | "turn_end"
            | "message_start"
            | "message_update"
            | "message_end"
            | "set_model"
            | "set_thinking_level"
            | "session_compact"
            | "session_compact_failed"
            | "session_shutdown"
            | "mcp_servers_change"
            | "before_agent_start"
            | "tool_call"
            | "tool_result"
            | "context"
            | "context_with_system"
            | "input"
            | "user_bash"
            | "session_before_compact"
            | "command"
            | "shortcut"
            | "extension_tool_call"
            | "provider_stream"
    )
}

/// Run one core→extension command. Returns the response line to emit
/// (`None` = fire-and-forget, or `provider_stream`, whose ack + events
/// are emitted inside).
async fn run_command<E: Extension>(b: &Arc<Bridge>, ext: &E, pi: &Pi, v: Value) -> Option<Value> {
    let rtype = v.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let id = v.get("id").and_then(Value::as_str).map(str::to_string);
    let ctx = b.ctx_snapshot();

    match rtype {
        // ---- fire-and-forget events ----
        "session_start" => {
            if let Ok(ev) = event_of::<SessionStartEvent>(&v) {
                if b.subscribed("session_start") {
                    ext.on_session_start(&ev, &ctx);
                }
            }
            None
        }
        "agent_start" => {
            if let Ok(ev) = event_of::<AgentStartEvent>(&v) {
                if b.subscribed("agent_start") {
                    ext.on_agent_start(&ev, &ctx);
                }
            }
            None
        }
        "agent_end" => {
            if let Ok(ev) = event_of::<AgentEndEvent>(&v) {
                if b.subscribed("agent_end") {
                    ext.on_agent_end(&ev, &ctx);
                }
            }
            None
        }
        "turn_start" => {
            if let Ok(ev) = event_of::<TurnStartEvent>(&v) {
                if b.subscribed("turn_start") {
                    ext.on_turn_start(&ev, &ctx);
                }
            }
            None
        }
        "turn_end" => {
            if let Ok(ev) = event_of::<TurnEndEvent>(&v) {
                if b.subscribed("turn_end") {
                    ext.on_turn_end(&ev, &ctx);
                }
            }
            None
        }
        "message_start" => {
            if let Ok(ev) = event_of::<MessageStartEvent>(&v) {
                if b.subscribed("message_start") {
                    ext.on_message_start(&ev, &ctx);
                }
            }
            None
        }
        "message_update" => {
            if let Ok(ev) = event_of::<MessageUpdateEvent>(&v) {
                if b.subscribed("message_update") {
                    ext.on_message_update(&ev, &ctx);
                }
            }
            None
        }
        "message_end" => {
            if let Ok(ev) = event_of::<MessageEndEvent>(&v) {
                if b.subscribed("message_end") {
                    ext.on_message_end(&ev, &ctx);
                }
            }
            None
        }
        "session_compact" => {
            if let Ok(ev) = event_of::<SessionCompactEvent>(&v) {
                if b.subscribed("session_compact") {
                    ext.on_session_compact(&ev, &ctx);
                }
            }
            None
        }
        "session_compact_failed" => {
            if let Ok(ev) = event_of::<SessionCompactFailedEvent>(&v) {
                if b.subscribed("session_compact_failed") {
                    ext.on_session_compact_failed(&ev, &ctx);
                }
            }
            None
        }
        "set_model" => {
            if let Ok(ev) = event_of::<ModelSelectEvent>(&v) {
                if b.subscribed("set_model") {
                    ext.on_model_select(&ev, &ctx);
                }
                // The sidecar's view tracks the selection regardless of
                // subscriptions.
                b.ctx.lock().unwrap().model = Some(ev.model.clone());
            }
            None
        }
        "set_thinking_level" => {
            if let Ok(ev) = event_of::<ThinkingLevelSelectEvent>(&v) {
                if b.subscribed("set_thinking_level") {
                    ext.on_thinking_level_select(&ev, &ctx);
                }
                b.ctx.lock().unwrap().thinking_level = Some(ev.level.clone());
            }
            None
        }
        "mcp_servers_change" => {
            if let Some(ev) = v
                .get("event")
                .cloned()
                .and_then(|e| serde_json::from_value::<McpServersChangeEvent>(e).ok())
            {
                if b.subscribed("mcp_servers_change") {
                    ext.on_mcp_servers_change(&ev, &ctx);
                }
            }
            None
        }
        // `session_shutdown`: flag first (queued commands are dropped),
        // emit only when subscribed, then exit 0 (sidecar order).
        "session_shutdown" => {
            b.shutting_down.store(true, Ordering::SeqCst);
            if let Ok(ev) = event_of::<SessionShutdownEvent>(&v) {
                if b.subscribed("session_shutdown") {
                    ext.on_session_shutdown(&ev, &ctx);
                }
            }
            b.exit_flush(0).await
        }

        // ---- decision commands (fast path: no handlers → data null) ----
        "before_agent_start" => {
            let data = if b.subscribed("before_agent_start") {
                match event_of::<BeforeAgentStartEvent>(&v) {
                    Ok(ev) => match ext.on_before_agent_start(&ev, &ctx) {
                        Some(r) => serde_json::to_value(r).unwrap(),
                        // No additions, no prompt change: the empty object
                        // (sidecar shape).
                        None => Value::Object(Default::default()),
                    },
                    Err(e) => {
                        b.emit_error("before_agent_start", &malformed_payload(&e));
                        Value::Object(Default::default())
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "before_agent_start",
                true,
                Some(data),
                None,
            ))
        }
        "tool_call" => {
            // The response carries the final input (the Rust handler
            // cannot mutate it, so it is always echoed back unchanged).
            let data = if b.subscribed("tool_call") {
                match event_of::<ToolCallEvent>(&v) {
                    Ok(ev) => {
                        let r = ext.on_tool_call(&ev, &ctx);
                        json!({ "result": r, "input": ev.input })
                    }
                    Err(e) => {
                        b.emit_error("tool_call", &malformed_payload(&e));
                        json!({ "result": null, "input": v["event"]["input"] })
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "tool_call",
                true,
                Some(data),
                None,
            ))
        }
        "tool_result" => {
            let data = if b.subscribed("tool_result") {
                match event_of::<ToolResultEvent>(&v) {
                    Ok(ev) => serde_json::to_value(ext.on_tool_result(&ev, &ctx)).unwrap(),
                    Err(e) => {
                        b.emit_error("tool_result", &malformed_payload(&e));
                        Value::Null
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "tool_result",
                true,
                Some(data),
                None,
            ))
        }
        // v0 no-op: all handler phases run once on context_with_system
        // (sidecar contract — `context` is answered without running
        // anything).
        "context" => Some(response_line(
            id.as_deref(),
            "context",
            true,
            Some(Value::Null),
            None,
        )),
        "context_with_system" => {
            if !b.subscribed("context") && !b.subscribed("context_with_system") {
                return Some(response_line(
                    id.as_deref(),
                    "context_with_system",
                    true,
                    Some(Value::Null),
                    None,
                ));
            }
            let data = match event_of::<ContextWithSystemEvent>(&v) {
                Ok(ev) => {
                    let mut current = ev.messages.clone();
                    // Phase 1: `context` handlers see the conversation
                    // without system messages; the leading system message
                    // is restored (upstream `restoreSystemMessages`).
                    if b.subscribed("context") {
                        let visible: Vec<AgentMessage> = current
                            .iter()
                            .filter(|m| !matches!(m, AgentMessage::System(_)))
                            .cloned()
                            .collect();
                        if let Some(returned) = ext.on_context(
                            &ContextEvent {
                                kind: "context".into(),
                                messages: visible.clone(),
                            },
                            &ctx,
                        ) {
                            if returned != visible {
                                let head = current
                                    .first()
                                    .cloned()
                                    .filter(|m| matches!(m, AgentMessage::System(_)));
                                current = match head {
                                    Some(h) => std::iter::once(h).chain(returned).collect(),
                                    None => returned,
                                };
                            }
                        }
                    }
                    // Phase 2: the full transcript; the output is used as-is.
                    if b.subscribed("context_with_system") {
                        if let Some(new_msgs) = ext.on_context_with_system(
                            &ContextWithSystemEvent {
                                kind: "context_with_system".into(),
                                messages: current.clone(),
                            },
                            &ctx,
                        ) {
                            current = new_msgs;
                        }
                    }
                    serde_json::to_value(current).unwrap()
                }
                Err(e) => {
                    b.emit_error("context_with_system", &malformed_payload(&e));
                    v["event"]["messages"].clone()
                }
            };
            Some(response_line(
                id.as_deref(),
                "context_with_system",
                true,
                Some(data),
                None,
            ))
        }
        "input" => {
            let data = if b.subscribed("input") {
                match event_of::<InputEvent>(&v) {
                    Ok(ev) => match ext.on_input(&ev, &ctx) {
                        Some(r) => serde_json::to_value(r).unwrap(),
                        None => json!({ "action": "continue" }),
                    },
                    Err(e) => {
                        b.emit_error("input", &malformed_payload(&e));
                        json!({ "action": "continue" })
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "input",
                true,
                Some(data),
                None,
            ))
        }
        "user_bash" => {
            let data = if b.subscribed("user_bash") {
                match event_of::<UserBashEvent>(&v) {
                    Ok(ev) => serde_json::to_value(ext.on_user_bash(&ev, &ctx)).unwrap(),
                    Err(e) => {
                        b.emit_error("user_bash", &malformed_payload(&e));
                        Value::Null
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "user_bash",
                true,
                Some(data),
                None,
            ))
        }
        "session_before_compact" => {
            let data = if b.subscribed("session_before_compact") {
                match event_of::<SessionBeforeCompactEvent>(&v) {
                    Ok(ev) => {
                        serde_json::to_value(ext.on_session_before_compact(&ev, &ctx)).unwrap()
                    }
                    Err(e) => {
                        b.emit_error("session_before_compact", &malformed_payload(&e));
                        Value::Null
                    }
                }
            } else {
                Value::Null
            };
            Some(response_line(
                id.as_deref(),
                "session_before_compact",
                true,
                Some(data),
                None,
            ))
        }

        // ---- req/resp commands ----
        "command" => {
            let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = v
                .get("args")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match pi.commands().iter().find(|c| c.name == name) {
                None => Some(response_line(
                    id.as_deref(),
                    "command",
                    false,
                    None,
                    Some(format!("Unknown command: {name}")),
                )),
                Some(c) => {
                    let r = (c.handler)(args, &ctx).await;
                    Some(response_line(id.as_deref(), "command", true, r, None))
                }
            }
        }
        "shortcut" => {
            let raw = v.get("key").and_then(Value::as_str).unwrap_or_default();
            let key = raw.to_lowercase();
            // Upstream keys the shortcut map by the lowercased key; the
            // stored key keeps its registered case.
            match pi.shortcuts().iter().find(|s| s.key.to_lowercase() == key) {
                None => Some(response_line(
                    id.as_deref(),
                    "shortcut",
                    false,
                    None,
                    Some(format!("No extension shortcut for key: {raw}")),
                )),
                Some(s) => {
                    (s.handler)(&ctx).await;
                    Some(response_line(
                        id.as_deref(),
                        "shortcut",
                        true,
                        Some(Value::Null),
                        None,
                    ))
                }
            }
        }
        "extension_tool_call" => {
            let name = v
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = v.get("input").cloned().unwrap_or_else(|| json!({}));
            match pi.tools().iter().find(|t| t.name == name) {
                None => Some(response_line(
                    id.as_deref(),
                    "extension_tool_call",
                    false,
                    None,
                    Some(format!("Unknown extension tool: {name}")),
                )),
                Some(t) => {
                    let tc = ToolContext::new(&ctx, &*pi.transport);
                    let r = (t.execute)(input, &tc).await;
                    // A tool failure is an isError result (upstream
                    // `AgentTool.execute` converts throws the same way).
                    let data = match r {
                        Ok(result) => result,
                        Err(msg) => AgentToolResult::error(msg),
                    };
                    Some(response_line(
                        id.as_deref(),
                        "extension_tool_call",
                        true,
                        Some(serde_json::to_value(data).unwrap()),
                        None,
                    ))
                }
            }
        }
        "provider_stream" => provider_stream(b, pi, v).await,

        // Unknown command types are filtered by dispatch_line.
        _ => None,
    }
}

/// The `event` field of a command line, typed.
fn event_of<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, serde_json::Error> {
    let ev = v.get("event").cloned().unwrap_or(Value::Null);
    serde_json::from_value(ev)
}

fn malformed_payload(e: &serde_json::Error) -> String {
    format!("malformed event payload: {e}")
}

/// `provider_stream`: validate, ack, then forward every upstream event
/// as `provider_stream_event`; the terminal `done`/`error` ends the
/// stream (a stream that ends without one gets a typed error event).
async fn provider_stream(b: &Arc<Bridge>, pi: &Pi, v: Value) -> Option<Value> {
    let id = v.get("id").and_then(Value::as_str).map(str::to_string);
    let ack_err = |msg: String| {
        Some(response_line(
            id.as_deref(),
            "provider_stream",
            false,
            None,
            Some(msg),
        ))
    };
    let stream_id = v
        .get("streamId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| next_id("stream"));
    let model = match v
        .get("model")
        .cloned()
        .and_then(|m| serde_json::from_value::<ModelRef>(m).ok())
    {
        Some(m) if !m.provider.is_empty() && !m.id.is_empty() => m,
        _ => return ack_err("provider_stream needs a model with provider and id".into()),
    };
    let provider = match pi.providers().iter().find(|p| p.name == model.provider) {
        Some(p) => p,
        None => return ack_err(format!("unknown extension provider: {}", model.provider)),
    };
    if !provider.config.models.iter().any(|m| m.id == model.id) {
        return ack_err(format!(
            "extension provider {} has no model {}",
            provider.name, model.id
        ));
    }
    let messages = v
        .get("messages")
        .cloned()
        .and_then(|m| serde_json::from_value::<Vec<AgentMessage>>(m).ok())
        .unwrap_or_default();
    let options = v
        .get("options")
        .cloned()
        .and_then(|o| serde_json::from_value::<ProviderStreamOptions>(o).ok())
        .unwrap_or_default();
    // Ack first, then stream (sidecar order).
    b.emit(&response_line(
        id.as_deref(),
        "provider_stream",
        true,
        Some(json!({ "ok": true, "streamId": stream_id })),
        None,
    ));
    let terminal = std::sync::atomic::AtomicBool::new(false);
    let mut emit = |ev: AssistantMessageEvent| {
        if matches!(
            ev,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        ) {
            terminal.store(true, Ordering::SeqCst);
        }
        b.emit_stream_event(&stream_id, &ev);
    };
    (provider.config.stream)(model.clone(), messages, &options, &mut emit).await;
    if !terminal.load(Ordering::SeqCst) {
        emit(provider_error_event(
            &model,
            "provider stream ended without a terminal event",
        ));
    }
    None
}

/// The terminal `error` event for a failed provider stream (upstream
/// `providerErrorEvent` shape: `stopReason "error"` + `errorMessage`).
fn provider_error_event(model: &ModelRef, error: &str) -> AssistantMessageEvent {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    AssistantMessageEvent::Error {
        reason: "error".into(),
        error: AgentMessage::Assistant(AssistantMessage {
            content: Vec::new(),
            api: model.api.clone(),
            provider: Some(model.provider.clone()),
            model: Some(model.id.clone()),
            usage: Some(crate::wire::Usage {
                input: Some(0),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
                total_tokens: Some(0),
                ..Default::default()
            }),
            stop_reason: Some("error".into()),
            error_message: Some(error.to_string()),
            timestamp: Some(Value::from(now)),
            ..Default::default()
        }),
    }
}

// =====================================================================
// the v6 transport (pi actions) + UI + pane host
// =====================================================================

/// The [`ExtensionTransport`] for the v6 process: the wire-expressible
/// actions cross as extension→core lines, `exec` runs a local child
/// process (the isolation boundary is this process; no exec line exists
/// in the v6 catalog), `set_label` has no wire line (v0).
pub struct BridgeTransport(Arc<Bridge>);

impl ExtensionTransport for BridgeTransport {
    fn send_user_message(&self, content: MessageContent, _options: Option<SendUserMessageOptions>) {
        // `user_message { text, images? }` (F&F) — the core enqueues the
        // turn (steers when one is active).
        let (text, images) = match content {
            MessageContent::Text(t) => (t, None),
            MessageContent::Blocks(blocks) => {
                let text = blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let images: Vec<ContentBlock> = blocks
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::Image { .. }))
                    .cloned()
                    .collect();
                (text, (!images.is_empty()).then_some(images))
            }
        };
        self.0
            .emit_typed(&SidecarEvent::UserMessage { text, images });
    }

    fn append_entry(&self, custom_type: &str, data: Value) {
        // The core is the source of truth: it fills id/parentId/timestamp.
        self.0.emit_typed(&SidecarEvent::SessionAppendEntry {
            entry: json!({ "type": "custom", "customType": custom_type, "data": data }),
        });
    }

    fn set_session_name(&self, name: &str) {
        self.0
            .emit_typed(&SidecarEvent::SessionInfoChanged(SessionInfoChanged {
                name: Some(name.to_string()),
            }));
    }

    fn set_label(&self, _entry_id: &str, _label: Option<&str>) {
        // No wire line for labels in the v6 catalog (the sidecar keeps a
        // local view; v0).
        eprintln!("[pi-ext] setLabel has no wire line in the v6 catalog (v0)");
    }

    fn get_settings(&self) -> Value {
        Value::Object(Default::default())
    }

    fn set_thinking_level(&self, level: &str) {
        let b = self.0.clone();
        let level = level.to_string();
        tokio::spawn(async move {
            let resp = b
                .request("set_thinking_level_request", json!({ "level": level }))
                .await;
            if resp.get("success") != Some(&Value::Bool(true))
                || resp.get("data").and_then(|d| d.get("ok")) != Some(&Value::Bool(true))
            {
                eprintln!(
                    "[pi-ext] set_thinking_level_request for \"{level}\" not applied by core"
                );
            }
        });
    }

    fn set_model(&self, model: ModelRef) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let b = self.0.clone();
        Box::pin(async move {
            let resp = b
                .request(
                    "set_model_request",
                    json!({ "modelId": model.id, "provider": model.provider }),
                )
                .await;
            if resp.get("success") == Some(&Value::Bool(true))
                && resp.get("data").and_then(|d| d.get("ok")) == Some(&Value::Bool(true))
            {
                true
            } else {
                eprintln!(
                    "[pi-ext] set_model_request for {}/{} not applied by core",
                    model.provider, model.id
                );
                false
            }
        })
    }

    fn exec(
        &self,
        command: &str,
        args: &[String],
    ) -> Pin<Box<dyn Future<Output = ExecResult> + Send + '_>> {
        // A local child process (upstream: the sidecar's Node process;
        // here: the extension's own process — the same isolation).
        let command = command.to_string();
        let args = args.to_vec();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                match std::process::Command::new(&command).args(&args).output() {
                    Ok(o) => ExecResult {
                        stdout: String::from_utf8_lossy(&o.stdout).into(),
                        stderr: String::from_utf8_lossy(&o.stderr).into(),
                        // shortcut: std does not expose the signal number
                        // portably; signal-killed reads as code 1, <when
                        // the std API grows, map 128+signal>.
                        code: o.status.code().unwrap_or(1),
                        killed: false,
                        truncated: false,
                    },
                    Err(e) => ExecResult {
                        stdout: String::new(),
                        stderr: e.to_string(),
                        code: 1,
                        killed: false,
                        truncated: false,
                    },
                }
            })
            .await
            .unwrap_or(ExecResult {
                stdout: String::new(),
                stderr: "exec task failed".into(),
                code: 1,
                killed: false,
                truncated: false,
            })
        })
    }

    fn execute_tool(
        &self,
        name: &str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, String>> + Send + '_>> {
        let b = self.0.clone();
        let name = name.to_string();
        Box::pin(async move {
            let tool_call_id = next_id("tc");
            let resp = b
                .request(
                    "tool_call_request",
                    json!({ "toolCallId": tool_call_id, "toolName": name, "input": args }),
                )
                .await;
            if resp.get("success") == Some(&Value::Bool(true)) {
                serde_json::from_value::<AgentToolResult>(
                    resp.get("data").cloned().unwrap_or(Value::Null),
                )
                .map_err(|e| format!("tool_call_request response is not an AgentToolResult: {e}"))
            } else {
                Err(resp
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_call_request failed")
                    .to_string())
            }
        })
    }

    fn ui(&self) -> Arc<dyn ExtensionUI> {
        Arc::new(BridgeUi(self.0.clone()))
    }
}

/// The [`ExtensionUI`] for the v6 process: the dialogs round-trip as
/// `extension_ui_request` / `extension_ui_response` with the
/// capability timeout; the fire-and-forget methods always emit; a
/// method the core does not advertise resolves to the upstream default
/// (headless = the noOp UI context).
struct BridgeUi(Arc<Bridge>);

impl ExtensionUI for BridgeUi {
    fn select(
        &self,
        title: &str,
        options: &[String],
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        let b = self.0.clone();
        let title = title.to_string();
        let options = options.to_vec();
        let timeout = opts.timeout_ms;
        Box::pin(async move {
            if !b.ui_supported("select") {
                return None;
            }
            b.send_dialog(&UiRequest::Select {
                id: next_id("d"),
                title,
                options,
                timeout,
            })
            .await
            .and_then(|r| {
                if r.get("cancelled") == Some(&Value::Bool(true)) {
                    None
                } else {
                    r.get("value").and_then(Value::as_str).map(str::to_string)
                }
            })
        })
    }

    fn confirm(
        &self,
        title: &str,
        message: &str,
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let b = self.0.clone();
        let title = title.to_string();
        let message = message.to_string();
        let timeout = opts.timeout_ms;
        Box::pin(async move {
            if !b.ui_supported("confirm") {
                return false;
            }
            b.send_dialog(&UiRequest::Confirm {
                id: next_id("d"),
                title,
                message,
                timeout,
            })
            .await
            .map(|r| {
                r.get("cancelled") != Some(&Value::Bool(true))
                    && r.get("confirmed") == Some(&Value::Bool(true))
            })
            .unwrap_or(false)
        })
    }

    fn input(
        &self,
        title: &str,
        placeholder: Option<&str>,
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        let b = self.0.clone();
        let title = title.to_string();
        let placeholder = placeholder.map(str::to_string);
        let timeout = opts.timeout_ms;
        Box::pin(async move {
            if !b.ui_supported("input") {
                return None;
            }
            b.send_dialog(&UiRequest::Input {
                id: next_id("d"),
                title,
                placeholder,
                timeout,
            })
            .await
            .and_then(|r| {
                if r.get("cancelled") == Some(&Value::Bool(true)) {
                    None
                } else {
                    r.get("value").and_then(Value::as_str).map(str::to_string)
                }
            })
        })
    }

    fn editor(
        &self,
        title: &str,
        prefill: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        let b = self.0.clone();
        let title = title.to_string();
        let prefill = prefill.map(str::to_string);
        Box::pin(async move {
            if !b.ui_supported("editor") {
                return None;
            }
            // No per-call timeout (upstream rpc-mode): the negotiated
            // default applies (send_dialog falls back to it).
            b.send_dialog(&UiRequest::Editor {
                id: next_id("d"),
                title,
                prefill,
            })
            .await
            .and_then(|r| {
                if r.get("cancelled") == Some(&Value::Bool(true)) {
                    None
                } else {
                    r.get("value").and_then(Value::as_str).map(str::to_string)
                }
            })
        })
    }

    fn notify(&self, message: &str, kind: Option<NotifyKind>) {
        // F&F, always emitted (the core drops it when unsupported).
        self.0
            .emit_typed(&SidecarEvent::ExtensionUiRequest(UiRequest::Notify {
                id: next_id("d"),
                message: message.to_string(),
                notify_type: kind.map(|k| k.as_str().to_string()),
            }));
    }

    fn custom(
        &self,
        factory: crate::pi::PaneFactory,
        blocked: Option<PaneBlocked>,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + '_>> {
        let b = self.0.clone();
        Box::pin(async move {
            // No `custom` capability → the upstream no-UI default
            // (the factory is never run).
            if !b.ui_supported("custom") {
                return None;
            }
            let id = next_id("p");
            let (done_tx, mut done_rx) = oneshot::channel::<Value>();
            b.panes.lock().unwrap().insert(
                id.clone(),
                PaneState {
                    waiters: Mutex::new(VecDeque::new()),
                    done: Mutex::new(Some(done_tx)),
                    opened: AtomicBool::new(false),
                    closed: AtomicBool::new(false),
                    blocked,
                    result: Mutex::new(None),
                },
            );
            let timeout = *b.timeout_ms.lock().unwrap();
            let pane = Pane::new(Arc::new(BridgePaneHost(b.clone())), id.clone());
            // The factory's future IS the component's lifetime (sidecar:
            // the factory builds the component, the host drives it; here
            // the component drives itself through the `Pane` handle).
            // The sidecar emits `pane_open` only after a successful
            // factory — so the Rust pane opens lazily on the component's
            // first host call (`update`/`next_key`), and a factory that
            // fails before that emits no `pane_open`.
            let factory_task = tokio::spawn(async move {
                let _ = factory(pane).await;
            });
            let abort = factory_task.abort_handle();
            // The pane ends with `done(result)` — the component's end
            // signal (sidecar: done resolves the local promise and
            // disposes the component; a task lingering after done is
            // aborted, not awaited) — or a factory panic
            // (`extension_error`, the Rust panic protocol), a factory
            // return without done (the component ended), or the
            // capability timeout (the upstream default, `None`).
            let result = tokio::select! {
                _ = &mut done_rx => {
                    // done fired: `BridgePaneHost::done` emitted
                    // `pane_close` and stored the result before
                    // resolving.
                    b.pane_result(&id)
                }
                t = factory_task => match t {
                    Err(je) if je.is_panic() => {
                        // done had already ended the pane (a post-done
                        // panic): the done result wins — no close (done
                        // emitted it), no error (the sidecar disposes
                        // the component on done).
                        let r = b.pane_result(&id);
                        if r.is_none() {
                            b.emit_error(
                                "ui.custom",
                                &format!("pane factory panicked: {}", take_last_panic()),
                            );
                        }
                        b.emit_pane_close_once(&id);
                        r
                    }
                    Ok(()) => {
                        // The component's future completed: if done fired
                        // a moment before the return, its result wins
                        // (close already emitted by done).
                        let r = b.pane_result(&id);
                        b.emit_pane_close_once(&id);
                        r
                    }
                    Err(_) => None, // cancelled: nothing left to close
                },
                _ = tokio::time::sleep(std::time::Duration::from_millis(timeout)) => {
                    // done may have won the same instant: its result
                    // wins (no timeout log for it).
                    let r = b.pane_result(&id);
                    if r.is_none() {
                        eprintln!("[pi-ext] pane {id} resolved to the timeout default");
                    }
                    b.emit_pane_close_once(&id);
                    r
                }
            };
            // No-op when the task already finished (done / panic /
            // return); kills a task lingering after done.
            abort.abort();
            // Drop the pane state: later pane_* lines for the id are
            // ignored.
            b.panes.lock().unwrap().remove(&id);
            result
        })
    }

    fn set_status(&self, key: &str, text: Option<&str>) {
        self.0
            .emit_typed(&SidecarEvent::ExtensionUiRequest(UiRequest::SetStatus {
                id: next_id("d"),
                status_key: key.to_string(),
                status_text: text.map(str::to_string),
            }));
    }

    fn set_title(&self, title: &str) {
        self.0
            .emit_typed(&SidecarEvent::ExtensionUiRequest(UiRequest::SetTitle {
                id: next_id("d"),
                title: title.to_string(),
            }));
    }

    fn set_editor_text(&self, text: &str) {
        self.0.emit_typed(&SidecarEvent::ExtensionUiRequest(
            UiRequest::SetEditorText {
                id: next_id("d"),
                text: text.to_string(),
            },
        ));
    }
}

/// The [`PaneHost`] for the v6 process: `pane_update` out, `pane_key`
/// in, `pane_close` on `done`.
struct BridgePaneHost(Arc<Bridge>);

impl PaneHost for BridgePaneHost {
    fn update(&self, id: &str, desc: &PaneDescriptor) {
        if !self.0.open_pane(id) {
            return; // pane gone (timeout/done): no-op
        }
        self.0.emit_typed(&SidecarEvent::PaneUpdate {
            id: id.to_string(),
            title: desc.title.clone(),
            lines: desc.lines.clone(),
            list: desc.list.clone(),
            input: desc.input.clone(),
            blocked: desc.blocked.clone(),
        });
    }

    fn next_key(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = Option<PaneKeyInput>> + Send + '_>> {
        if !self.0.open_pane(id) {
            return Box::pin(async { None });
        }
        let (tx, rx) = oneshot::channel::<PaneKeyInput>();
        if let Some(st) = self.0.panes.lock().unwrap().get(id) {
            st.waiters.lock().unwrap().push_back(tx);
        }
        Box::pin(async move { rx.await.ok() })
    }

    fn done(&self, id: &str, result: Value) {
        let tx = {
            let panes = self.0.panes.lock().unwrap();
            let Some(st) = panes.get(id) else {
                return; // pane already ended (timeout): no-op
            };
            let Some(tx) = st.done.lock().unwrap().take() else {
                return; // second `done`: no-op
            };
            // Remember the result before resolving, so the select can
            // read it even if the factory task finishes at the same
            // instant.
            *st.result.lock().unwrap() = Some(result.clone());
            tx
        };
        // The component ended the pane: emit `pane_close` (the core
        // drops the pane) before resolving the local promise.
        self.0.emit_pane_close_once(id);
        let _ = tx.send(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug-fix pin (7): duplicate shortcut keys dedupe on the
    /// lowercased key (upstream Map semantics) — the later registration
    /// wins + the conflict diagnostic (position kept, like Map.set).
    #[test]
    fn resolve_shortcuts_dedupes_lowercased_keys() {
        let s1 = Shortcut::new("Ctrl+D", |_ctx: &ExtensionContext| Box::pin(async {}));
        let s2 = Shortcut::new("ctrl+d", |_ctx: &ExtensionContext| Box::pin(async {}));
        let (resolved, diags) = resolve_shortcuts(&[s1, s2], &BTreeMap::new(), "ext-a");
        assert_eq!(resolved.len(), 1, "deduped: {resolved:?}");
        assert_eq!(resolved[0].key, "ctrl+d", "the later registration wins");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].contains("registered by both"), "{diags:?}");
    }

    /// Bug-fix pin (6): a dialog timeout releases the `dialogs` entry
    /// (request() removes on timeout the same way) — no leak, a late
    /// `extension_ui_response` reads "unknown dialog".
    #[tokio::test]
    async fn dialog_timeout_releases_its_entry() {
        let b = Bridge::start(
            tokio::io::sink(),
            ExtensionContext::default(),
            "test".into(),
        );
        b.set_capabilities(vec!["confirm".into()], 30_000);
        let req = UiRequest::Confirm {
            id: "d-1".into(),
            title: "t".into(),
            message: "m".into(),
            timeout: Some(30),
        };
        assert!(
            b.send_dialog(&req).await.is_none(),
            "timeout -> upstream default"
        );
        assert!(
            b.dialogs.lock().unwrap().is_empty(),
            "the dialogs entry must not leak"
        );
    }
}
