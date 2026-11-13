use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::client::{BridgeClient, Decision, State};
use crate::model::{AssistantTurn, FakeDriver, ModelDriver, StreamDelta};
use crate::provider::ProviderDriver;
use crate::session::{now_ms, Session};
use crate::tools;

/// Base system prompt for v0 fake-model runs.
const BASE_SYSTEM_PROMPT: &str = "You are pi-rust, a minimal Rust agent core for the extension-bridge PoC. Use the provided tools.";

/// A queued user turn: scenario input ("rpc") or a sidecar `user_message` ("extension").
pub struct QueuedUser {
    pub text: String,
    pub source: &'static str,
    pub steps: Vec<Value>,
    /// (mime, base64) `--image` attachments (S8).
    pub images: Vec<(String, String)>,
    /// WP-704: the fake driver's compaction summary (scenario `compact_summary`).
    pub compact_summary: String,
}

pub enum Item {
    User(QueuedUser),
    End,
}

type Ack = Option<tokio::sync::oneshot::Sender<()>>;

#[derive(Clone)]
pub struct EventLog {
    tx: Option<mpsc::UnboundedSender<(Value, Ack)>>,
    /// Lossless in-process event stream (TUI): a lagging broadcast receiver
    /// silently drops events — under load that loses extension statuses
    /// (CI macOS found it). mpsc: the single TUI consumer never loses one.
    sub: mpsc::UnboundedSender<Value>,
    sub_rx: std::sync::Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
}

impl EventLog {
    /// `headless` = NDJSON on stdout (the core output contract for
    /// --scenario/--prompt/--list-sessions runs); the TUI renders in-process
    /// instead and passes `false`.
    pub fn start(headless: bool) -> Self {
        let (sub, sub_rx) = mpsc::unbounded_channel::<Value>();
        let tx = if headless {
            let (tx, mut rx) = mpsc::unbounded_channel::<(Value, Ack)>();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let mut out = tokio::io::BufWriter::new(tokio::io::stdout());
                while let Some((v, ack)) = rx.recv().await {
                    let _ = out.write_all(format!("{}\n", v.to_string().trim_end()).as_bytes()).await;
                    let _ = out.flush().await;
                    if let Some(a) = ack {
                        let _ = a.send(());
                    }
                }
            });
            Some(tx)
        } else {
            None
        };
        Self {
            tx,
            sub,
            sub_rx: std::sync::Arc::new(std::sync::Mutex::new(Some(sub_rx))),
        }
    }

    /// Subscribe to the in-process event stream (TUI, exactly once).
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Value> {
        self.sub_rx
            .lock()
            .unwrap()
            .take()
            .expect("the TUI event stream is subscribed exactly once")
    }


    /// TUI-only event (in-process broadcast only — never on the headless
    /// NDJSON stdout, so the machine output contract stays unchanged).
    pub fn emit_tui(&self, v: Value) {
        let _ = self.sub.send(v);
    }

    pub fn emit(&self, v: Value) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((v.clone(), None));
        }
        let _ = self.sub.send(v);
    }

    /// Emits and (headless) waits until the line is written and flushed (the
    /// final transcript line must reach stdout before the core exits).
    pub async fn emit_final(&self, v: Value) {
        if let Some(tx) = &self.tx {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            if tx.send((v.clone(), Some(ack_tx))).is_ok() {
                let _ = ack_rx.await;
            }
        }
        let _ = self.sub.send(v);
    }

    /// One line per decision event: name + request + response.
    pub fn decision(&self, name: &str, req: &Value, resp: &Value) {
        self.emit(json!({"kind":"decision","name":name,"request":req,"response":resp}));
    }
}

/// Shared state of the agent loop and the sidecar-event processor.
/// WP-610: one-shot stdout variants (upstream print-mode parity).
/// `Ndjson` is the default core contract (kind events on stdout);
/// `Text` writes only the final assistant text; `Json` writes the
/// upstream-shaped AgentSessionEvent stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputMode {
    Ndjson,
    Text,
    Json,
    /// WP-802: upstream `--mode rpc` surface (stdin JSONL command loop).
    Rpc,
}

pub struct Core {
    pub client: Arc<BridgeClient>,
    /// WP-610: the one-shot stdout variant (headless only).
    pub out_mode: OutputMode,
    /// WP-610: Text mode — the last assistant text of the run.
    pub text_final: Arc<Mutex<Option<String>>>,
    /// WP-610: Json mode — upstream-shaped event lines to stdout.
    pub json_tx: Option<mpsc::UnboundedSender<(String, Option<tokio::sync::oneshot::Sender<()>>)>>,
    pub session: Arc<Session>,
    pub events: Arc<Mutex<HashSet<String>>>,            // ready.events (subscribed event names)
    pub ext_tools: Arc<Mutex<Vec<Value>>>,              // ready.tools entries (pass-through schemas)
    /// WP-701: extension-registered models keyed `provider/id` (ready +
    /// providers_changed payloads); per-turn bridge routing + set_model gate.
    pub ext_models: Arc<Mutex<std::collections::BTreeMap<String, Value>>>,
    /// WP-801: skills discovered at startup (system-prompt section + /skill: expansion).
    pub skills: Arc<Vec<crate::skills::Skill>>,    pub ready: Mutex<Option<tokio::sync::oneshot::Sender<Value>>>, // full `ready` payload → cli
    pub log: EventLog,
    pub fail: Arc<Mutex<Option<String>>>,               // first fatal scenario error
    pub model: Arc<Mutex<(Value, Option<String>)>>,     // (active model, thinking level)
    pub msg_tx: mpsc::UnboundedSender<Item>,
    pub done: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Live provider driver (None = scripted fake driver per scenario run).
    /// Mutex: runtime setModel (WP-406) swaps it between turns.
    pub provider: Arc<Mutex<Option<ProviderDriver>>>,
    /// Agent dir for runtime credential re-resolution (WP-406).
    pub agent_dir: Arc<std::path::PathBuf>,
    /// Cumulative token usage / cost for --print-usage (WP-407).
    pub usage_total: Arc<Mutex<crate::model::Usage>>,
    /// WP-802: true while an agent run is executing (rpc `prompt` guard).
    pub run_active: Arc<std::sync::atomic::AtomicBool>,
    pub cost_total: Arc<Mutex<f64>>,
    /// TUI mode (WP-604): blocking ui dialogs are forwarded to the TUI
    /// instead of answering with the upstream defaults.
    pub tui: bool,
    /// Dialog requests for the TUI (WP-604); headless keeps it unconnected.
    pub ui_tx: mpsc::UnboundedSender<Value>,
    /// WP-702: drain barrier for event_task — `sync_tool_set` sends a ping
    /// and waits; the ping is answered only after every sidecar line that
    /// reached the event channel earlier (in particular a `tools_changed`
    /// emitted while the MCP extension's `tool_call` handler waited for a
    /// server) has been applied.
    pub tools_sync_tx: mpsc::UnboundedSender<tokio::sync::oneshot::Sender<()>>,
    /// WP-704: the scenario `compact` override (window, reserve, keep) — the
    /// fake model has no catalog contextWindow, so without the override the
    /// auto trigger stays off for it.
    pub compact_override: Option<(u64, u64, u64)>,
}

/// WP-610: Json mode — write one upstream-shaped event line.
fn jemit(ctx: &Core, v: Value) {
    if ctx.out_mode == OutputMode::Json || ctx.out_mode == OutputMode::Rpc {
        let _ = ctx.json_tx.as_ref().unwrap().send((v.to_string(), None));
    }
}

/// WP-610: Json mode — the streamed text delta as an upstream
/// `message_update` (the `partial` snapshot is dropped, per toJsonEvent).
fn json_delta(ctx: &Core, content_index: u32, delta: &str) {
    jemit(ctx, json!({"type":"message_update","usage":json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0}),
        "assistantMessageEvent":{"type":"text_delta","contentIndex":content_index,"delta":delta}}));
}

/// True when the core may send the event: it must appear in `ready.events` and
/// in the core catalog. `context_with_system` doubles as the two-phase context
/// hook — the single upstream emitContext runs there (sidecar resolution 4), so
/// a subscription to `context` also routes through it.
fn subscribed(ctx: &Core, name: &str) -> bool {
    let ev = ctx.events.lock().unwrap();
    ev.contains(name) || (name == "context_with_system" && ev.contains("context"))
}

/// WP-702: wait until `event_task` has applied every sidecar line that
/// preceded the in-flight blocking command's response. The sidecar writes a
/// `tools_changed` line and the decision response in that order on one pipe, and
/// the demux delivers them in that order — but the decision response can wake
/// the agent loop before `event_task` drains the earlier event, so the tool
/// set must not be read without this barrier after a decision that may have
/// connected an MCP server (`tool_call`, `before_agent_start`).
async fn sync_tool_set(ctx: &Core) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    if ctx.tools_sync_tx.send(tx).is_ok() {
        let _ = rx.await;
    }
}

/// Response `data` of a successful decision, else the protocol default
/// (`fallback`); logs the decision event line either way.
fn decision_data(ctx: &Core, resp: Decision, name: &str, req: &Value, fallback: &Value) -> Value {
    let data = match resp {
        Decision::Response(r) if r.get("success") == Some(&json!(true)) => r.get("data").cloned().unwrap_or(Value::Null),
        Decision::Response(r) => {
            crate::corelog::line(&format!("[core] {name} decision error: {}", r.get("error").and_then(Value::as_str).unwrap_or("?")));
            fallback.clone()
        }
        Decision::Unavailable => fallback.clone(),
    };
    ctx.log.decision(name, req, &data);
    data
}

/// Sidecar→core event processor. Runs on its own task so a nested
/// `tool_call_request` can be answered while the agent loop awaits a decision
/// (bidirectional liveness, no head-of-line blocking).
pub async fn event_task(
    mut events: mpsc::UnboundedReceiver<Value>,
    mut tools_sync: mpsc::UnboundedReceiver<tokio::sync::oneshot::Sender<()>>,
    ctx: Arc<Core>,
) {
    loop {
        // The events branch is listed FIRST: a queued sidecar event is
        // processed before a drain ping is answered, which is what makes
        // `sync_tool_set` a real "everything written before the ping is
        // applied" barrier (WP-702).
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match ev.get("type").and_then(Value::as_str).unwrap_or("") {
                "ready" => {
                    let empty = Vec::new();
                    let names: Vec<String> = ev.get("events").and_then(Value::as_array).unwrap_or(&empty)
                        .iter().filter_map(Value::as_str).map(str::to_string).collect();
                    ctx.events.lock().unwrap().extend(names.iter().cloned());
                    ctx.ext_tools.lock().unwrap().clone_from(ev.get("tools").and_then(Value::as_array).unwrap_or(&empty));
                    // WP-701: extension-registered models become routable.
                    *ctx.ext_models.lock().unwrap() = parse_providers(&ev);
                    crate::corelog::line(&format!(
                        "[core] ready: {} extension(s), {} extension tool(s), events: {names:?}",
                        ev.get("extensions").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
                        ctx.ext_tools.lock().unwrap().len()
                    ));
                    if let Some(r) = ctx.ready.lock().unwrap().take() {
                        let _ = r.send(ev);
                    }
                }
                "extension_ui_request" => {
                    let method = ev.get("method").and_then(Value::as_str).unwrap_or("");
                    if ctx.tui && matches!(method, "select" | "confirm" | "input" | "editor") {
                        // WP-604: the TUI renders the dialog and answers it over
                        // the unchanged bridge protocol.
                        if ctx.ui_tx.send(ev.clone()).is_err() {
                            ui_default(&ctx, &ev); // TUI gone: fall back to defaults.
                        }
                    } else if ctx.tui {
                        // F&F UI channel: display-only, the TUI renders it.
                        let kind = match method {
                            "notify" => "notify",
                            "setStatus" => "set_status",
                            "setTitle" => "set_title",
                            "set_editor_text" => "set_editor_text",
                            _ => "extension_ui_request",
                        };
                        ctx.log.emit(json!({"kind":kind,
                            "id":ev.get("id").cloned().unwrap_or(Value::Null),
                            "method":method,
                            "title":ev.get("title").cloned().unwrap_or(Value::Null),
                            "message":ev.get("message").cloned().unwrap_or(Value::Null),
                            "statusKey":ev.get("statusKey").cloned().unwrap_or(Value::Null),
                            "statusText":ev.get("statusText").cloned().unwrap_or(Value::Null),
                            "text":ev.get("text").cloned().unwrap_or(Value::Null),
                            "response":Value::Null}));
                    } else {
                        ui_default(&ctx, &ev);
                    }
                }
                "extension_error" => {
                    crate::corelog::line(&format!("[core] extension_error: {ev}"));
                    ctx.log.emit(json!({"kind":"extension_error","extensionPath":ev.get("extensionPath").cloned().unwrap_or(Value::Null),
                        "event":ev.get("event").cloned().unwrap_or(Value::Null),"error":ev.get("error").cloned().unwrap_or(Value::Null)}));
                }
                "session_info_changed" => {
                    let name = ev.get("name").and_then(Value::as_str).map(str::to_string);
                    ctx.session.set_name(name.clone());
                    crate::corelog::line(&format!("[core] session name: {name:?}"));
                }
                "session_append_entry" => {
                    if let Some(entry) = ev.get("entry") {
                        ctx.session.append_entry(entry.clone());
                    }
                }
                "user_message" => {
                    let text = ev.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                    crate::corelog::line(&format!("[core] extension user_message: {text}"));
                    // Steer for v0 = append to the same queue (drained in order).
                    let _ = ctx.msg_tx.send(Item::User(QueuedUser { text, source: "extension", steps: Vec::new(), images: Vec::new(), compact_summary: crate::model::DEFAULT_COMPACT_SUMMARY.to_string() }));
                }
                "providers_changed" => {
                    // WP-701: a registration arrived after ready; re-key routing.
                    let next = parse_providers(&ev);
                    crate::corelog::line(&format!("[core] providers_changed: {} extension model(s)", next.len()));
                    *ctx.ext_models.lock().unwrap() = next;
                }
                "tools_changed" => {
                    // WP-702: the extension tool set changed post-ready (MCP
                    // tools register on connection, lazy reconnect, unregister)
                    // — replace the ready.tools snapshot.
                    let next = ev.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
                    crate::corelog::line(&format!("[core] tools_changed: {} extension tool(s)", next.len()));
                    ctx.ext_tools.lock().unwrap().clone_from(&next);
                }
                "set_model_request" => {
                    let id = ev.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let model_id = ev.get("modelId").and_then(Value::as_str).map(str::to_string);
                    let provider = ev.get("provider").and_then(Value::as_str).map(str::to_string);
                    // WP-406: validate first (catalog + driver build), only then
                    // swap; declines leave the active model untouched. IMPORTANT:
                    // no `return` here — a return would exit event_task, not just
                    // this arm, and kill the sidecar->core event pump.
                    let reply = |success: bool, data: Value| {
                        let mut line = json!({"id":id,"type":"response","command":"set_model_request","success":success});
                        if success { line["data"] = data; } else { line["error"] = data; }
                        ctx.client.send(line);
                    };
                    match (provider, model_id) {
                        (Some(p), Some(m)) => {
                            // WP-701: extension-registered model — routable via
                            // the bridge stream; no bundled driver is built.
                            let key = format!("{p}/{m}");
                            if ctx.ext_models.lock().unwrap().contains_key(&key) {
                                let prev = ctx.model.lock().unwrap().0.clone();
                                let model = json!({"id":m,"provider":p});
                                ctx.model.lock().unwrap().0 = model.clone();
                                ctx.session.set_model(&p, &m);
                                reply(true, json!({"ok":true}));
                                if subscribed(&ctx, "model_select") {
                                    ctx.client.fire(json!({"type":"set_model","event":{"type":"model_select","model":model,"previousModel":prev,"source":"set"}}));
                                }
                            } else if crate::registry::ModelRegistry::bundled().resolve(&p, &m).is_none() {
                                reply(false, json!(format!("unknown model {p}/{m}")));
                            } else {
                                match crate::provider::build_provider(&p, Some(&m), &ctx.agent_dir) {
                                    Ok(Some(drv)) => {
                                        let prev = ctx.model.lock().unwrap().0.clone();
                                        let model = json!({"id":m,"provider":p});
                                        ctx.model.lock().unwrap().0 = model.clone();
                                        ctx.session.set_model(&p, &m);
                                        *ctx.provider.lock().unwrap() = Some(drv);
                                        reply(true, json!({"ok":true}));
                                        if subscribed(&ctx, "model_select") {
                                            ctx.client.fire(json!({"type":"set_model","event":{"type":"model_select","model":model,"previousModel":prev,"source":"set"}}));
                                        }
                                    }
                                    Ok(None) => reply(false, json!(format!("provider {p} is not buildable at runtime (v0)"))),
                                    Err(e) => reply(false, json!(format!("cannot switch to {p}/{m}: {e}"))),
                                }
                            }
                        }
                        _ => reply(false, json!("setModel requires provider and modelId")),
                    }
                }
                "set_thinking_level_request" => {
                    let id = ev.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let level = ev.get("level").and_then(Value::as_str).map(str::to_string);
                    let prev = ctx.model.lock().unwrap().1.clone().unwrap_or_else(|| "off".into());
                    ctx.model.lock().unwrap().1 = level.clone();
                    if let Some(l) = &level {
                        ctx.session.set_thinking_level(l);
                    }
                    ctx.client.send(json!({"id":id,"type":"response","command":"set_thinking_level_request","success":true,"data":{"ok":level.is_some()}}));
                    if subscribed(&ctx, "thinking_level_select") {
                        ctx.client.fire(json!({"type":"set_thinking_level","event":{"type":"thinking_level_select","level":level,"previousLevel":prev}}));
                    }
                }
                "set_theme_request" => {
                    // WP-607: pi.setTheme from an extension. The TUI applies the
                    // switch via the `set_theme` broadcast; headless runs reject
                    // (upstream theme switching is an interactive-mode feature).
                    let id = ev.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                    let name = ev.get("name").and_then(Value::as_str).map(str::to_string);
                    let reply = |success: bool, data: Value| {
                        let mut line =
                            json!({"id":id,"type":"response","command":"set_theme_request","success":success});
                        if success {
                            line["data"] = data;
                        } else {
                            line["error"] = data;
                        }
                        ctx.client.send(line);
                    };
                    match name {
                        Some(n) if ctx.tui => match crate::tui::theme::Theme::load(&ctx.agent_dir, &n) {
                            Ok(t) => {
                                ctx.log.emit_tui(json!({"kind":"set_theme","name":t.name}));
                                reply(true, json!({"ok":true}));
                            }
                            Err(e) => reply(false, json!(e)),
                        },
                        Some(n) => reply(
                            false,
                            json!(format!(
                                "theme {n:?} cannot be switched outside interactive (TUI) mode"
                            )),
                        ),
                        None => reply(false, json!("setTheme requires a theme name")),
                    }
                }
                "tool_call_request" => nested_tool(&ctx, &ev).await,
                other => crate::corelog::line(&format!("[core] ignoring unknown sidecar event: {other}")),
                }
            }
            // WP-702 drain barrier: answer only after the events branch has
            // had priority (select polls branches in order).
            ping = tools_sync.recv() => {
                let Some(tx) = ping else { break };
                let _ = tx.send(());
            }
        }
    }
}

/// Headless default-answers path: blocking dialogs are answered with the
/// upstream defaults immediately (select/input/editor → cancelled, confirm →
/// false) and logged; F&F ui requests (notify, setStatus, ...) need no
/// reply. Every ui request is logged to the event log with its answer.
/// TUI mode (WP-604) does not use this path for dialogs: the TUI renders
/// them and answers with the user's choice.
fn ui_default(ctx: &Core, ev: &Value) {
    let (Some(id), Some(method)) = (ev.get("id").and_then(Value::as_str), ev.get("method").and_then(Value::as_str)) else {
        return;
    };
    let title = ev.get("title").and_then(Value::as_str).unwrap_or_default();
    crate::corelog::line(&format!("[core] extension ui '{method}' ({title}): upstream default applied (no human UI in v0)"));
    let resp = if method == "confirm" {
        json!({"type":"extension_ui_response","id":id,"confirmed":false})
    } else if matches!(method, "select" | "input" | "editor") {
        json!({"type":"extension_ui_response","id":id,"cancelled":true})
    } else {
        Value::Null // F&F methods: no reply line
    };
    if !resp.is_null() {
        ctx.client.send(resp.clone());
    }
    ctx.log.emit(json!({"kind":"extension_ui_request","id":id,"method":method,
        "title":ev.get("title").cloned().unwrap_or(Value::Null),
        "message":ev.get("message").cloned().unwrap_or(Value::Null),
        "response":resp}));
}

/// Sidecar→core `tool_call_request` (nested `ctx.executeTool`): execute the
/// named tool with the normal hooks and answer with the `AgentToolResult`.
/// The nested result is NOT appended to the main transcript (upstream: nested
/// calls appear only in the parent result's nestedCalls).
async fn nested_tool(ctx: &Core, ev: &Value) {
    let id = ev.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let call_id = ev.get("toolCallId").and_then(Value::as_str).unwrap_or("nested").to_string();
    let name = ev.get("toolName").and_then(Value::as_str).unwrap_or_default().to_string();
    let input = ev.get("input").cloned().unwrap_or_else(|| json!({}));
    let data = match execute_tool_call(ctx, &call_id, &name, &input, false).await {
        Ok((trm, _)) => {
            let mut d = json!({"content":trm.get("content").cloned().unwrap_or(json!([])),"isError":trm.get("isError").cloned().unwrap_or(json!(false))});
            if let Some(det) = trm.get("details") {
                d["details"] = det.clone();
            }
            if let Some(s) = trm.get("structuredContent") {
                d["structuredContent"] = s.clone();
            }
            d
        }
        Err(msg) => json!({"content":[{"type":"text","text":msg}],"details":{},"isError":true}),
    };
    ctx.client.send(json!({"id":id,"type":"response","command":"tool_call_request","success":true,"data":data}));
}

enum ToolFailure {
    /// The (fake) model script named a tool that is neither built-in nor
    /// registered by any extension → model script error, abort non-zero.
    Unknown(String),
    /// Execution-level failure → recorded as an error tool result.
    Other(String),
}

/// `tool_call` decision (execute with the FINAL returned input) → execute
/// (built-in locally / extension over the bridge) → `tool_result` decision.
/// Returns (toolResult message, terminate hint).
async fn execute_tool_call(ctx: &Core, call_id: &str, name: &str, input: &Value, barrier: bool) -> Result<(Value, bool), String> {
    let event = json!({"type":"tool_call","toolCallId":call_id,"toolName":name,"input":input});
    // Degraded default: no block, input unchanged.
    let data = decision_data(ctx, ctx.client.request("tool_call", json!({"event":event})).await, "tool_call", &event, &json!({"result":Value::Null,"input":input}));
    // The MCP extension's tool_call handler waits for the server's readiness
    // inside the decision; its `tools_changed` must be applied first. The
    // barrier is only meaningful for the agent-loop caller: event_task runs
    // events strictly in order, so its own queued `tools_changed` lines are
    // already applied when it reaches this point (and it cannot answer its
    // own ping — waiting here would self-deadlock the nested path).
    if barrier {
        sync_tool_set(ctx).await;
    }
    let blocked = data.pointer("/result/block").and_then(Value::as_bool).unwrap_or(false);
    let final_input = data.get("input").cloned().filter(|v| v.is_object()).unwrap_or_else(|| input.clone());
    let (content, details, is_error, terminate) = if blocked {
        let reason = data.pointer("/result/reason").and_then(Value::as_str).unwrap_or("no reason");
        (json!([{"type":"text","text":format!("Blocked by extension: {reason}")}]), json!({}), true,
         data.pointer("/result/terminate").and_then(Value::as_bool).unwrap_or(false))
    } else {
        match run_tool(ctx, call_id, name, &final_input).await {
            Ok(r) => (r.content, r.details, r.is_error, r.terminate),
            Err(ToolFailure::Unknown(n)) => return Err(format!("model script calls unknown tool '{n}' (neither built-in nor in ready.tools)")),
            Err(ToolFailure::Other(msg)) => (json!([{"type":"text","text":msg}]), json!({}), true, false),
        }
    };
    ctx.log.emit(json!({"kind":"tool_execution","name":name,"input":final_input,"blocked":blocked,"result_summary":summarize(&content)}));
    Ok(finalize_tool_result(ctx, call_id, name, &final_input, content, details, is_error, terminate).await)
}

async fn run_tool(ctx: &Core, call_id: &str, name: &str, input: &Value) -> Result<tools::ExecResult, ToolFailure> {
    // Upstream precedence: an extension tool shadows the built-in of the same name.
    let ext_hit = ctx.client.state() == State::Running
        && ctx.ext_tools.lock().unwrap().iter().any(|t| t.get("name").and_then(Value::as_str) == Some(name));
    // WP-702: MCP tool names (upstream `mcp__<server>__<tool>` scheme and the
    // resource tools) are bridge-routable even when the server never connected
    // (no entry in ext_tools): the sidecar answers "Unknown extension tool" and
    // the call records an error tool result instead of aborting the run as a
    // model-script error.
    let mcp_name = name.starts_with("mcp__")
        || matches!(name, "list_mcp_resources" | "list_mcp_resource_templates" | "read_mcp_resource");
    if !ext_hit && !mcp_name && tools::is_builtin(name) {
        return Ok(tools::execute(name, input).await);
    }
    if !ext_hit && !mcp_name && !tools::is_builtin(name) {
        return Err(ToolFailure::Unknown(name.to_string()));
    }
    // Extension tools are dropped from the model's tool set while degraded; a
    // scripted call then records an error result instead of crossing the bridge.
    if ctx.client.state() != State::Running {
        return Err(ToolFailure::Other(format!("extension tool '{name}' unavailable: bridge degraded")));
    }
    match ctx.client.request("extension_tool_call", json!({"toolCallId":call_id,"toolName":name,"input":input})).await {
        Decision::Response(r) if r.get("success") == Some(&json!(true)) => {
            let d = r.get("data").cloned().unwrap_or(Value::Null);
            Ok(tools::ExecResult {
                content: d.get("content").cloned().filter(|v| v.is_array()).unwrap_or_else(|| json!([{"type":"text","text":"(no content)"}])),
                details: d.get("details").cloned().unwrap_or(json!({})),
                is_error: d.get("isError").and_then(Value::as_bool).unwrap_or(false),
                terminate: d.get("terminate").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        Decision::Response(r) => Err(ToolFailure::Other(r.get("error").and_then(Value::as_str).unwrap_or("extension_tool_call failed").to_string())),
        Decision::Unavailable => Err(ToolFailure::Other("extension_tool_call unanswered: bridge degraded".into())),
    }
}

/// `tool_result` decision; applies the returned mutation to the recorded
/// result. Upstream rule: replacing `content` without `structuredContent`
/// drops the structured content.
async fn finalize_tool_result(ctx: &Core, call_id: &str, name: &str, input: &Value, content: Value, details: Value, is_error: bool, terminate: bool) -> (Value, bool) {
    let mut event = json!({"type":"tool_result","toolCallId":call_id,"toolName":name,"input":input,"content":content,"isError":is_error});
    if !details.is_null() {
        event["details"] = details.clone();
    }
    let data = decision_data(ctx, ctx.client.request("tool_result", json!({"event":event.clone()})).await, "tool_result", &event, &Value::Null);
    let mut result = json!({"role":"toolResult","toolCallId":call_id,"toolName":name,"content":content,"isError":is_error,"timestamp":now_ms()});
    if !details.is_null() {
        result["details"] = details.clone();
    }
    if !data.is_null() {
        if let Some(c) = data.get("content") {
            result["content"] = c.clone();
            if !data.get("structuredContent").is_some_and(|v| !v.is_null()) {
                result.as_object_mut().unwrap().remove("structuredContent");
            }
        }
        if let Some(d) = data.get("details") {
            result["details"] = d.clone();
        }
        if let Some(e) = data.get("isError").and_then(Value::as_bool) {
            result["isError"] = json!(e);
        }
        if let Some(s) = data.get("structuredContent") {
            if !s.is_null() {
                result["structuredContent"] = s.clone();
            }
        }
    }
    (result, terminate)
}

/// WP-603: the per-turn streaming callback — each text chunk becomes a
/// TUI-only `model_delta` event (never on the headless NDJSON stdout).
fn delta_emitter<'c>(ctx: &'c Core, turn_index: u32, rpc_skeleton: Arc<std::sync::Mutex<bool>>) -> Box<dyn FnMut(StreamDelta) + Send + 'c> {
    Box::new(move |ev: StreamDelta| {
        match &ev {
            StreamDelta::TextDelta { delta, .. } => {
                ctx.log
                    .emit_tui(json!({"kind":"model_delta","turnIndex":turn_index,"text":delta}));
                if ctx.out_mode == OutputMode::Json {
                    json_delta(ctx, 0, delta);
                }
            }
            _ => {}
        }
        // WP-802: Rpc mode carries the full upstream `message_update` event
        // set (text_start/delta/end, toolcall_start/delta/end); Json keeps
        // its shipped text_delta-only contract.
        if ctx.out_mode == OutputMode::Rpc {
            // The message_start arrives with the first stream delta
            // (upstream `start` event parity), not before the model call.
            if !*rpc_skeleton.lock().unwrap() {
                let items: Vec<Value> = match &ev {
                    StreamDelta::TextStart { .. } => vec![json!({"type":"text","text":""})],
                    StreamDelta::ToolCallStart { id, name, initial_args, .. } => {
                        let args = if initial_args.is_object() { initial_args.clone() } else { json!({}) };
                        vec![json!({"type":"toolCall","id":id,"name":name,"arguments":args})]
                    }
                    _ => Vec::new(),
                };
                let active = ctx.model.lock().unwrap().0.clone();
                let m = rpc_assistant_message(&Value::Array(items), &active, "stop",
                    &json!(crate::session::epoch_ms()), 0, true);
                jemit(ctx, json!({"type":"message_start","message":m}));
                *rpc_skeleton.lock().unwrap() = true;
            }
            let ame = match &ev {
                StreamDelta::TextStart { index } => json!({"type":"text_start","contentIndex":index}),
                StreamDelta::TextDelta { index, delta } => json!({"type":"text_delta","contentIndex":index,"delta":delta}),
                StreamDelta::TextEnd { index, content } => json!({"type":"text_end","contentIndex":index,"content":content}),
                StreamDelta::ToolCallStart { index, id, name, .. } => json!({"type":"toolcall_start","contentIndex":index,"id":id,"toolName":name}),
                StreamDelta::ToolCallDelta { index, delta } => json!({"type":"toolcall_delta","contentIndex":index,"delta":delta}),
                StreamDelta::ToolCallEnd { index, tool_call } => json!({"type":"toolcall_end","contentIndex":index,"toolCall":tool_call}),
            };
            jemit(ctx, json!({"type":"message_update","usage":rpc_usage6(&crate::model::Usage::default()),"assistantMessageEvent":ame}));
        }
    })
}

/// WP-802: upstream `Usage` wire shape (the 6-key form with `totalTokens`
/// and the nested `cost` object; values are zeroed here — the cumulative
/// totals live in the session usage entries).
fn rpc_usage6(u: &crate::model::Usage) -> Value {
    let total = u.input + u.output;
    json!({"input":u.input,"output":u.output,"cacheRead":u.cache_read,"cacheWrite":u.cache_write,
        "totalTokens":total,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}})
}

/// WP-802: upstream assistant-message wire shape (key structure matches the
/// pinned upstream `AssistantMessage`; pi values where pi differs).
fn rpc_assistant_message(content: &Value, active: &Value, stop_reason: &str, ts: &Value, duration_ms: u64, skeleton: bool) -> Value {
    let usage = content.get("usage").cloned().unwrap_or(json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0}));
    let total = usage.get("input").and_then(Value::as_u64).unwrap_or(0) + usage.get("output").and_then(Value::as_u64).unwrap_or(0);
    // The upstream wire tool-call items use {type,id,name,arguments}; the
    // stored compact form uses {id,input,name,type}.
    let content = match content {
        Value::Array(items) => Value::Array(items.iter().map(|it| {
            if it.get("type").and_then(Value::as_str) == Some("toolCall") && it.get("input").is_some() {
                json!({"type":"toolCall","id":it.get("id").cloned().unwrap_or(Value::Null),
                    "name":it.get("name").cloned().unwrap_or(Value::Null),
                    "arguments":it.get("input").cloned().unwrap_or(Value::Null)})
            } else { it.clone() }
        }).collect()),
        other => other.clone(),
    };
    let mut m = json!({"role":"assistant","content":content,"api":active.get("id").cloned().unwrap_or(Value::Null),
        "provider":active.get("provider").cloned().unwrap_or(Value::Null),"model":active.get("id").cloned().unwrap_or(Value::Null),
        "usage":{"input":usage.get("input").cloned().unwrap_or(json!(0)),"output":usage.get("output").cloned().unwrap_or(json!(0)),
            "cacheRead":usage.get("cacheRead").cloned().unwrap_or(json!(0)),"cacheWrite":usage.get("cacheWrite").cloned().unwrap_or(json!(0)),
            "totalTokens":total,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},
        "stopReason":stop_reason,"timestamp":ts.clone()});
    if !skeleton {
        // The upstream wire always carries the raw provider stop reason
        // (tool_calls -> toolUse is the only mapping pi v0 produces).
        m["rawStopReason"] = json!(if stop_reason == "toolUse" {"tool_calls"} else {stop_reason});
        m["thinkingLevel"] = json!("off");
        m["durationMs"] = json!(duration_ms);
    }
    m
}

/// WP-802: the upstream system-message wire shape emitted around every run
/// (key structure only — the prompt section values are pi's own).
fn rpc_system_message(ctx: &Core) -> Value {
    let mut sections = json!({"preamble":BASE_SYSTEM_PROMPT,
        "cwd":std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()});
    if !ctx.skills.is_empty() {
        sections["skills"] = json!(crate::skills::format_for_prompt(&ctx.skills));
    }
    let tools_added: Vec<Value> = merged_tool_defs(ctx).iter().map(|t| {
        json!({"name":t.get("name").cloned().unwrap_or(Value::Null),
            "description":t.get("description").cloned().unwrap_or(Value::Null),
            "parameters":t.get("parameters").cloned().unwrap_or(json!({})),
            "constrainedSampling":{"type":"json_schema","strict":"prefer"}})
    }).collect();
    json!({"role":"system","content":"","sections":sections,"timestamp":now_ms(),"toolsAdded":tools_added})
}

pub async fn agent_task(ctx: Arc<Core>, mut items: mpsc::UnboundedReceiver<Item>) {
    let mut index = 0u32;
    loop {
        let Some(item) = items.recv().await else { break };
        match item {
            Item::End => break,
            Item::User(u) => {
                index += 1;
                ctx.log.emit(json!({"kind":"user_input","index":index,"source":u.source,"text":u.text}));
                ctx.run_active.store(true, std::sync::atomic::Ordering::SeqCst);
                let err = match run_agent(&ctx, &u).await {
                    Ok(()) => None,
                    Err(msg) => Some(msg),
                };
                ctx.run_active.store(false, std::sync::atomic::Ordering::SeqCst);
                if let Some(msg) = err {
                    crate::corelog::line(&format!("[core] agent run failed: {msg}"));
                    // WP-610: Json mode surfaces the failure in-stream (the
                    // upstream error assistant message); Text mode stays
                    // silent on stdout (the error goes to stderr + exit 1).
                    if ctx.out_mode == OutputMode::Json || ctx.out_mode == OutputMode::Rpc {
                        jemit(&ctx, json!({"type":"message_end","message":{"role":"assistant","content":[],
                            "stopReason":"error","errorMessage":msg,"timestamp":crate::session::now_iso()}}));
                        jemit(&ctx, json!({"type":"agent_end","messages":[],"willRetry":false}));
                        jemit(&ctx, json!({"type":"agent_settled","aborted":false}));
                    }
                    *ctx.fail.lock().unwrap() = Some(msg);
                    break; // remaining queued items are dropped (v0)
                }
            }
        }
    }
    if let Some(d) = ctx.done.lock().unwrap().take() {
        let _ = d.send(());
    }
}

/// One agent run: `input` decision → user message → `before_agent_start` →
/// `agent_start` → turns (`context_with_system` → model output events → tool
/// calls → `turn_end`) → `agent_end`.
async fn run_agent(ctx: &Core, u: &QueuedUser) -> Result<(), String> {
    // The fake driver is stateful (script queue) and built once per run; live
    // provider drivers are re-read per turn so a runtime setModel (WP-406)
    // applies to the next turn.
    let fake_driver: Option<Box<dyn ModelDriver>> = match ctx.provider.lock().unwrap().as_ref() {
        Some(_) => None,
        None => {
            if u.steps.is_empty() {
                // No scripted model: deterministic echo turn (prompt runs and
                // TUI messages alike; the fake driver is a test double).
                Some(Box::new(FakeDriver::echo_mode(&u.compact_summary)))
            } else {
                Some(Box::new(FakeDriver::from_steps(&u.steps, &u.compact_summary).map_err(|e| e.to_string())?))
            }
        }
    };

    // `input` decision: transforms chain; "handled" drops the input (no turn).
    let mut text = u.text.clone();
    if subscribed(ctx, "input") {
        let event = json!({"type":"input","text":u.text,"source":u.source});
        let data = decision_data(ctx, ctx.client.request("input", json!({"event":event.clone()})).await, "input", &event, &json!({"action":"continue"}));
        match data.get("action").and_then(Value::as_str) {
            Some("handled") => {
                crate::corelog::line(&format!("[core] input handled by extension; no agent turn"));
                return Ok(());
            }
            Some("transform") => {
                if let Some(t) = data.get("text").and_then(Value::as_str) {
                    text = t.to_string();
                }
            }
            _ => {} // null / continue → original input
        }
    }

    let mut user_content: Vec<Value> = vec![json!({"type":"text","text":text})];
    for (mime, data) in &u.images {
        user_content.push(json!({"type":"image","mimeType":mime,"data":data}));
    }
    let user_msg = json!({"role":"user","content":user_content,"timestamp":crate::session::now_iso()});
    ctx.session.append_message(&user_msg);
    let is_rpc = ctx.out_mode == OutputMode::Rpc;
    // WP-802: in Rpc mode the user message events follow agent_start +
    // turn_start (upstream order); other modes keep the early start.
    if !is_rpc {
        jemit(ctx, json!({"type":"message_start","message":user_msg.clone()}));
    }

    // `before_agent_start`: prepend returned custom messages; a returned
    // systemPrompt (present only when a handler changed it) replaces the prompt
    // for this run.
    let mut custom: Vec<Value> = Vec::new();
    // WP-801: the skills section is part of the base prompt (upstream builds
    // it before before_agent_start); a handler-returned systemPrompt replaces
    // the whole prompt including the section.
    let mut system_prompt = crate::skills::append_section(BASE_SYSTEM_PROMPT, &ctx.skills);
    if subscribed(ctx, "before_agent_start") {
        let event = json!({"type":"before_agent_start","messageType":"user","prompt":text});
        // BuildSystemPromptOptions subset for the sidecar: core cwd, currently
        // active built-in tool names, empty sections (handlers mutate in place).
        let selected_tools: Vec<String> = merged_tool_defs(ctx)
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        let spon = json!({
            "cwd": std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            "selectedTools": selected_tools,
            "sections": {},
        });
        let req = json!({"event":event,"systemPrompt":system_prompt.clone(),"systemPromptOptions":spon});
        let data = decision_data(ctx, ctx.client.request("before_agent_start", req.clone()).await, "before_agent_start", &req, &Value::Null);
        if let Some(ms) = data.get("messages").and_then(Value::as_array) {
            custom = ms.to_vec();
        }
        if let Some(p) = data.get("systemPrompt").and_then(Value::as_str) {
            system_prompt = p.to_string();
        }
        // The MCP extension's before_agent_start handler waits for servers with
        // `direct` tools; its `tools_changed` must be applied before the turn's
        // tool declarations are built.
        sync_tool_set(ctx).await;
    }

    if subscribed(ctx, "agent_start") { ctx.client.fire(json!({"type":"agent_start","event":{"type":"agent_start"}})); }
    ctx.log.emit(json!({"kind":"agent_start"}));
    jemit(ctx, json!({"type":"agent_start"}));
    let run_start = ctx.session.transcript().len();
    // WP-704: the run's compaction settings (None = auto-compaction off).
    let compaction = compaction_settings(ctx);

    let mut turn_index = 0u32;
    loop {
        // WP-704: auto compaction at the context threshold — checked before
        // build_conversation so the summarized conversation is what this
        // turn's model call sees; after a successful compaction the loop
        // continues with the projected transcript.
        if let Some((window, reserve, keep)) = compaction {
            let system_msg = json!({"role":"system","content":system_prompt,"toolsAdded":merged_tool_defs(ctx),"timestamp":now_ms()});
            let check_convo = build_conversation(ctx, &system_prompt, &custom);
            let context_tokens = crate::session::estimate_context_tokens(&check_convo);
            if crate::session::should_compact(context_tokens, window, reserve) {
                let entries = ctx.session.entries();
                if let Some(pre) = crate::session::prepare_compaction(&entries, &system_msg, keep) {
                    run_compaction(ctx, fake_driver.as_ref(), &system_msg, &pre).await;
                } else {
                    crate::corelog::line("[core] compaction threshold reached, nothing to summarize (session too small)");
                }
            }
        }
        // `context` always answers data:null; the single emitContext runs on
        // `context_with_system` — send the full transcript and use the returned
        // conversation as-is for the provider request (sidecar may transform it).
        let mut convo = build_conversation(ctx, &system_prompt, &custom);
        if subscribed(ctx, "context_with_system") {
            let event = json!({"type":"context_with_system","messages":convo.clone()});
            let resp = decision_data(ctx, ctx.client.request("context_with_system", json!({"event":event.clone()})).await, "context_with_system", &event, &Value::Null);
            if let Some(ms) = resp.as_array() {
                convo = ms.clone();
            }
        }

        // WP-610: turn_start now precedes the model call (the upstream
        // order; the events carry the same payload).
        if subscribed(ctx, "turn_start") { ctx.client.fire(json!({"type":"turn_start","event":{"type":"turn_start","turnIndex":turn_index,"timestamp":now_ms()}})); }
        ctx.log.emit(json!({"kind":"turn_start","turnIndex":turn_index}));
        jemit(ctx, json!({"type":"turn_start"}));
        // WP-802: upstream order for the first turn — the run's initial
        // messages (system, user) stream after agent_start + turn_start.
        if is_rpc && turn_index == 0 {
            let sys = rpc_system_message(ctx);
            jemit(ctx, json!({"type":"message_start","message":sys.clone()}));
            jemit(ctx, json!({"type":"message_end","message":sys}));
            // The wire user message uses the upstream epoch-ms timestamp.
            let user_wire = json!({"role":"user","content":user_msg.get("content").cloned().unwrap_or(Value::Null),
                "timestamp":crate::session::epoch_ms()});
            jemit(ctx, json!({"type":"message_start","message":user_wire.clone()}));
            jemit(ctx, json!({"type":"message_end","message":user_wire}));
        }
        // WP-610: the assistant message_start (skeleton) opens the stream
        // before the driver call — the deltas land between start and end.
        if ctx.out_mode == OutputMode::Json {
            let active = ctx.model.lock().unwrap().0.clone();
            jemit(ctx, json!({"type":"message_start","message":{"role":"assistant",
                "content":[],"usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
                "api":active.get("id").cloned().unwrap_or(Value::Null),
                "provider":active.get("provider").cloned().unwrap_or(Value::Null),
                "model":active.get("id").cloned().unwrap_or(Value::Null)}}));
        }
        // WP-802: the RPC message_start lands with the first stream delta;
        // turns without deltas get it right before the message_end.
        let rpc_skeleton = Arc::new(std::sync::Mutex::new(false));
        let model_start = Instant::now();

        // WP-603: streamed text chunks become TUI-only `model_delta` events
        // (the headless NDJSON contract keeps its single `model_output`).
        let turn = model_call(ctx, fake_driver.as_ref(), &convo, delta_emitter(ctx, turn_index, rpc_skeleton.clone())).await?;

        // Usage accounting (WP-407): per-turn session entry + cumulative
        // total; cost from the bundled registry (unknown model → 0).
        if turn.usage != crate::model::Usage::default() {
            let active = ctx.model.lock().unwrap().0.clone();
            let p = active.get("provider").and_then(Value::as_str).unwrap_or("fake").to_string();
            let m = active.get("id").and_then(Value::as_str).unwrap_or("fake").to_string();
            let cost = turn
                .usage
                .cost_usd(crate::registry::ModelRegistry::bundled().resolve(&p, &m).map(|x| &x.cost));
            ctx.session.set_usage(&turn.usage, cost);
            ctx.usage_total.lock().unwrap().add(&turn.usage);
            *ctx.cost_total.lock().unwrap() += cost;
        }
        // The fake model does not stream: message_start + message_end only.
        let assistant = assistant_message(&turn, &ctx.model.lock().unwrap().0);
        if subscribed(ctx, "message_start") { ctx.client.fire(json!({"type":"message_start","event":{"type":"message_start","message":assistant}})); }
        if subscribed(ctx, "message_end") { ctx.client.fire(json!({"type":"message_end","event":{"type":"message_end","message":assistant}})); }
        // WP-610: message_end carries the final assistant message (the
        // skeleton message_start + the on_delta updates came first).
        let msg_id = ctx.session.append_message(&assistant);
        // WP-802: Rpc carries the upstream wire shape (full usage/cost,
        // thinkingLevel, durationMs); other modes keep the stored shape.
        let assistant_wire = if is_rpc {
            let stop = assistant.get("stopReason").and_then(Value::as_str).unwrap_or("stop");
            let wire = rpc_assistant_message(&assistant.get("content").cloned().unwrap_or(json!([])),
                &ctx.model.lock().unwrap().0, stop, &json!(crate::session::epoch_ms()),
                model_start.elapsed().as_millis() as u64, false);
            // A turn without stream deltas never emitted the message_start:
            // it lands here (upstream emits a start at stream start).
            if !*rpc_skeleton.lock().unwrap() {
                jemit(ctx, json!({"type":"message_start","message":wire.clone()}));
            }
            wire
        } else {
            assistant.clone()
        };
        jemit(ctx, json!({"type":"message_end","message":assistant_wire.clone()}));
        ctx.log.emit(json!({"kind":"model_output","turnIndex":turn_index,"text":turn.text.as_deref(),
            "toolCalls":turn.tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()}));
        if ctx.out_mode == OutputMode::Text {
            if let Some(t) = &turn.text {
                *ctx.text_final.lock().unwrap() = Some(t.clone());
            }
        }

        let mut tool_results: Vec<Value> = Vec::new();
        // WP-802: Rpc wire tool-result shapes for turn_end/agent_end.
        let mut tool_result_wires: Vec<Value> = Vec::new();
        let mut tool_result_ids: Vec<String> = Vec::new();
        let mut all_terminate = true;
        for tc in &turn.tool_calls {
            // WP-610: upstream tool_execution_start before the run (args).
            jemit(ctx, json!({"type":"tool_execution_start","toolCallId":tc.id,"toolName":tc.name,"args":tc.input}));
            if is_rpc {
                // WP-802: the execution stream opens with an empty partial
                // result (upstream update parity for the v0 non-streaming
                // tool surface).
                jemit(ctx, json!({"type":"tool_execution_update","toolCallId":tc.id,"toolName":tc.name,
                    "args":tc.input,"partialResult":{"content":[]}}));
            }
            let tool_start = Instant::now();
            match execute_tool_call(ctx, &tc.id, &tc.name, &tc.input, true).await {                Ok((trm, terminate)) => {
                    tool_result_ids.push(ctx.session.append_message(&trm));
                    // WP-610: upstream tool_execution_end (result + isError);
                    // WP-802: Rpc carries the full result shape and the
                    // measured durationMs.
                    let is_error = trm.get("content").and_then(Value::as_array)
                        .map(|c| c.iter().any(|x| x.get("isError") == Some(&json!(true))))
                        .unwrap_or(false);
                    let dur = tool_start.elapsed().as_millis() as u64;
                    let content = trm.get("content").cloned().unwrap_or(json!([]));
                    let mut te = if is_rpc {
                        jemit(ctx, json!({"type":"tool_execution_update","toolCallId":tc.id,"toolName":tc.name,
                            "args":tc.input,"partialResult":{"content":content.clone(),"details":{}}}));
                        json!({"type":"tool_execution_end","toolCallId":tc.id,"toolName":tc.name,
                            "result":{"content":content,
                                "structuredContent":trm.get("details").cloned().unwrap_or(json!({}))},
                            "isError":is_error,"durationMs":dur})
                    } else {
                        json!({"type":"tool_execution_end","toolCallId":tc.id,"toolName":tc.name,
                            "result":content,"isError":is_error})
                    };
                    jemit(ctx, te);
                    // WP-802: the toolResult message is a message event too
                    // (upstream emits message_start/end for it as well);
                    // the wire shape drops the stored `details` and carries
                    // the measured duration.
                    let trm_wire = if is_rpc {
                        json!({"role":"toolResult","toolCallId":tc.id,"toolName":tc.name,
                            "content":trm.get("content").cloned().unwrap_or(json!([])),
                            "isError":is_error,"durationMs":dur,
                            "timestamp":trm.get("timestamp").cloned().unwrap_or(json!(crate::session::epoch_ms()))})
                    } else {
                        trm.clone()
                    };
                    if is_rpc { tool_result_wires.push(trm_wire.clone()); }
                    jemit(ctx, json!({"type":"message_start","message":trm_wire.clone()}));
                    jemit(ctx, json!({"type":"message_end","message":trm_wire}));
                    tool_results.push(trm);
                    if !terminate {
                        all_terminate = false;
                    }
                }
                Err(msg) => return Err(msg), // unknown tool = model script error
            }
        }

        if subscribed(ctx, "turn_end") {
            ctx.client.fire(json!({"type":"turn_end","event":{"type":"turn_end","turnIndex":turn_index,"message":assistant,"toolResults":tool_results,
                "messageEntryId":msg_id,"toolResultEntryIds":tool_result_ids,"entries":[],"continue":true,"outcome":"completed",
                "context":{"contextEntries":[],"contextMessages":[],"llmMessages":[],"pendingMessages":[],"canContinue":false}}}));
        }
        ctx.log.emit(json!({"kind":"turn_end","turnIndex":turn_index}));
        // WP-802: the json/rpc stream carries turn_end (upstream
        // AgentSessionEvent parity; Rpc uses the full wire shape).
        let turn_msg = if is_rpc { assistant_wire.clone() } else { assistant.clone() };
        let turn_results = if is_rpc { tool_result_wires.clone() } else { tool_results.clone() };
        jemit(ctx, json!({"type":"turn_end","message":turn_msg,"toolResults":turn_results}));

        // A turn with no tool calls ends the run; `terminate` is honored only
        // when EVERY finalized result in the batch carried it (upstream rule).
        if turn.tool_calls.is_empty() || (all_terminate && !tool_results.is_empty()) {
            break;
        }
        turn_index += 1;
    }

    let run_messages = ctx.session.transcript()[run_start..].to_vec();
    // WP-802: upstream agent_end lists the run's messages starting with the
    // system message (then the user message); Rpc only.
    let wire_messages = if is_rpc {
        let mut v = vec![rpc_system_message(ctx), user_msg.clone()];
        v.extend(run_messages.iter().cloned());
        v
    } else {
        run_messages.clone()
    };
    if subscribed(ctx, "agent_end") { ctx.client.fire(json!({"type":"agent_end","event":{"type":"agent_end","messages":run_messages.clone()}})); }
    ctx.log.emit(json!({"kind":"agent_end"}));
    jemit(ctx, json!({"type":"agent_end","messages":wire_messages,"willRetry":false}));
    // WP-802: the client's idle gate (waitForIdle/promptAndWait) waits for
    // agent_settled after every run (Rpc only — the Json mode contract
    // ends at agent_end).
    if is_rpc {
        jemit(ctx, json!({"type":"agent_settled","aborted":false}));
    }
    Ok(())
}

/// One model call with the active driver: extension-registered model (bridge
/// stream, WP-701) → scripted fake driver → live provider. Shared by the turn
/// loop (streaming deltas) and the WP-704 summary call (no-op deltas).
async fn model_call<'a>(
    ctx: &'a Core,
    fake_driver: Option<&'a Box<dyn ModelDriver>>,
    convo: &[Value],
    mut deltas: Box<dyn FnMut(crate::model::StreamDelta) + Send + 'a>,
) -> Result<AssistantTurn, String> {
    // WP-701: an extension-registered model always routes via the
    // bridge — an explicit provider selection wins over the scripted
    // fake driver and the live provider driver.
    let active = ctx.model.lock().unwrap().0.clone();
    let p = active.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
    let mid = active.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let ext_model = ctx.ext_models.lock().unwrap().get(&format!("{p}/{mid}")).cloned();
    match ext_model {
        Some(model) => {
            let stream_id = uuid::Uuid::new_v4().to_string();
            let rx = ctx.client.attach_stream(&stream_id);
            let resp = ctx.client
                .request(
                    "provider_stream",
                    json!({"streamId":stream_id,"model":model,"messages":convo,"options":{}}),
                )
                .await;
            match resp {
                Decision::Response(r) if r.get("success") == Some(&json!(true)) => {
                    let mut events = Vec::new();
                    let mut rx = rx;
                    let mut text_started = false;
                    let mut text_full = String::new();
                    while let Some(line) = rx.recv().await {
                        if let Some(delta) = line.pointer("/event/delta").and_then(Value::as_str) {
                            // WP-802: the bridge streams text chunks; map them
                            // onto the StreamDelta event set (tool calls from
                            // extension models arrive in the event list, not
                            // the stream — M7 behavior unchanged).
                            if !text_started {
                                text_started = true;
                                deltas(crate::model::StreamDelta::TextStart { index: 0 });
                            }
                            text_full.push_str(delta);
                            deltas(crate::model::StreamDelta::TextDelta { index: 0, delta: delta.to_string() });
                        }
                        let is_terminal = line.get("event")
                            .and_then(|e| e.get("type"))
                            .and_then(Value::as_str)
                            .is_some_and(|t| t == "done" || t == "error");
                        events.push(line.get("event").cloned().unwrap_or(Value::Null));
                        if is_terminal {
                            if text_started {
                                deltas(crate::model::StreamDelta::TextEnd { index: 0, content: text_full.clone() });
                            }
                            break;
                        }
                    }
                    ctx.client.detach_stream(&stream_id);
                    provider_events_to_turn(&events).map_err(|e| e.to_string())
                }
                Decision::Response(r) => {
                    ctx.client.detach_stream(&stream_id);
                    Err(format!(
                        "{}",
                        r.get("error").and_then(Value::as_str).unwrap_or("provider_stream rejected")
                    ))
                }
                Decision::Unavailable => {
                    ctx.client.detach_stream(&stream_id);
                    Err("provider stream unavailable (sidecar degraded)".to_string())
                }
            }
        }
        None => match fake_driver {
            Some(d) => d.complete(convo, deltas).await.map_err(|e| e.to_string()),
            None => {
                let drv = ctx.provider.lock().unwrap().clone();
                match drv.as_ref() {
                    Some(ProviderDriver::OpenAi(d)) => d.complete(convo, deltas).await.map_err(|e| e.to_string()),
                    Some(ProviderDriver::Anthropic(d)) => d.complete(convo, deltas).await.map_err(|e| e.to_string()),
                    Some(ProviderDriver::Google(d)) => d.complete(convo, deltas).await.map_err(|e| e.to_string()),
                    Some(ProviderDriver::Bedrock(d)) => d.complete(convo, deltas).await.map_err(|e| e.to_string()),
                    None => unreachable!("fake_driver is None only while a provider exists"),
                }
            }
        },
    }
}

/// WP-704: the run's compaction settings — the scenario `compact` override
/// (window, reserve, keep) wins; else the bundled registry's contextWindow
/// with the upstream defaults. Models without a catalog contextWindow (like
/// the fake) keep auto-compaction off without the override.
fn compaction_settings(ctx: &Core) -> Option<(u64, u64, u64)> {
    if let Some(o) = ctx.compact_override {
        return Some(o);
    }
    let active = ctx.model.lock().unwrap().0.clone();
    let p = active.get("provider").and_then(Value::as_str).unwrap_or("").to_string();
    let mid = active.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let window = crate::registry::ModelRegistry::bundled().resolve(&p, &mid)?.context_window?;
    Some((window, crate::session::DEFAULT_RESERVE_TOKENS, crate::session::DEFAULT_KEEP_RECENT_TOKENS))
}

/// WP-704: the upstream Usage shape {input,output,cacheRead,cacheWrite} from
/// an extension-supplied compaction (None when absent or malformed).
fn extract_compaction_usage(v: Option<&Value>) -> Option<crate::model::Usage> {
    let v = v?;
    let f = |k: &str| v.get(k).and_then(Value::as_u64);
    let (Some(input), Some(output), Some(cache_read), Some(cache_write)) =
        (f("input"), f("output"), f("cacheRead"), f("cacheWrite"))
    else {
        return None;
    };
    Some(crate::model::Usage { input, output, cache_read, cache_write })
}

/// WP-704: the `session_compact_failed` F&F event (gated on ready.events).
/// v0: no AbortSignal exists, so `aborted` is always false; an extension
/// cancel is reported without an `errorMessage`.
fn fire_compact_failed(ctx: &Core, reason: &str, error_message: Option<&str>, from_extension: bool) {
    if !subscribed(ctx, "session_compact_failed") {
        return;
    }
    let mut event = json!({"type":"session_compact_failed","reason":reason,"aborted":false,"willRetry":false,"fromExtension":from_extension});
    if let Some(e) = error_message {
        event["errorMessage"] = json!(e);
    }
    ctx.client.fire(json!({"type":"session_compact_failed","event":event}));
}

/// WP-704: one compaction attempt. `session_before_compact` decision →
/// cancel (turn proceeds uncompacted) | extension summary (fromHook, no LLM
/// call) | LLM summary via the SAME active driver (verbatim prompts, no
/// toolsAdded, no message_start/end broadcast, no model_delta, no usage
/// session entry). On success: append_compaction + `session_compact` F&F.
/// A summary response with tool calls fails with a typed message (v0: the
/// drivers do not surface stopReason, so a length stop is not detectable).
async fn run_compaction(
    ctx: &Core,
    fake_driver: Option<&Box<dyn ModelDriver>>,
    system_msg: &Value,
    pre: &crate::session::CompactionPreparation,
) {
    {
        let reason = "threshold";
        let mut from_hook = false;
        let mut summary: Option<String> = None;
        let mut first_kept = pre.first_kept_entry_id.clone();
        let mut tokens_before = pre.tokens_before;
        let mut usage: Option<crate::model::Usage> = None;

        // session_before_compact decision (gated on ready.events).
        if subscribed(ctx, "session_before_compact") {
            let event = json!({"type":"session_before_compact",
                "preparation":{"reason":reason,"willRetry":false},
                "branchEntries":pre.messages_to_summarize});
            let req = json!({"event":event});
            let data = decision_data(ctx, ctx.client.request("session_before_compact", req.clone()).await, "session_before_compact", &req, &Value::Null);
            if data.get("cancel").and_then(Value::as_bool) == Some(true) {
                // Abort compaction; the turn proceeds uncompacted.
                crate::corelog::line("[core] compaction cancelled by session_before_compact");
                ctx.log.emit(json!({"kind":"compaction_failed","reason":reason,"cancelled":true,"fromExtension":false}));
                fire_compact_failed(ctx, reason, None, false);
                return;
            }
            if let Some(c) = data.get("compaction") {
                match c.get("summary").and_then(Value::as_str).map(str::to_string) {
                    Some(s) => {
                        // Extension-supplied summary: fromHook=true, skip the
                        // LLM call, use its firstKeptEntryId/tokensBefore when given.
                        from_hook = true;
                        summary = Some(s);
                        if let Some(fk) = c.get("firstKeptEntryId").and_then(Value::as_str) {
                            first_kept = fk.to_string();
                        }
                        if let Some(tb) = c.get("tokensBefore").and_then(Value::as_u64) {
                            tokens_before = tb;
                        }
                        usage = extract_compaction_usage(c.get("usage"));
                    }
                    None => {
                        let err = "session_before_compact returned a compaction without a summary string";
                        crate::corelog::line(&format!("[core] {err}"));
                        ctx.log.emit(json!({"kind":"compaction_failed","reason":reason,"error":err,"fromExtension":true}));
                        fire_compact_failed(ctx, reason, Some(err), true);
                        return;
                    }
                }
            }
        }

        let summary = match summary {
            Some(s) => s,
            None => {
                // LLM summary: the SAME active driver, conversation =
                // [system SUMMARIZATION_SYSTEM_PROMPT (no toolsAdded), user
                // "<conversation>…</conversation>" (+ <previous-summary>)].
                let base_prompt = pre
                    .previous_summary
                    .as_ref()
                    .map(|_| crate::session::UPDATE_SUMMARIZATION_PROMPT)
                    .unwrap_or(crate::session::SUMMARIZATION_PROMPT);
                let mut prompt_text = format!(
                    "<conversation>\n{}\n</conversation>\n\n",
                    crate::session::serialize_conversation(&pre.messages_to_summarize)
                );
                if let Some(prev) = &pre.previous_summary {
                    prompt_text.push_str(&format!("<previous-summary>\n{prev}\n</previous-summary>\n\n"));
                }
                prompt_text.push_str(base_prompt);
                let convo = vec![
                    json!({"role":"system","content":crate::session::SUMMARIZATION_SYSTEM_PROMPT,"timestamp":now_ms()}),
                    json!({"role":"user","content":[{"type":"text","text":prompt_text}],"timestamp":crate::session::now_iso()}),
                ];
                match model_call(ctx, fake_driver, &convo, Box::new(|_| {})).await {
                    Ok(turn) => {
                        if !turn.tool_calls.is_empty() {
                            let err = "Summarization attempted to call a tool";
                            crate::corelog::line(&format!("[core] {err}"));
                            ctx.log.emit(json!({"kind":"compaction_failed","reason":reason,"error":err,"fromExtension":false}));
                            fire_compact_failed(ctx, reason, Some(err), false);
                            return;
                        }
                        usage = (turn.usage != crate::model::Usage::default()).then_some(turn.usage);
                        turn.text.unwrap_or_default()
                    }
                    Err(msg) => {
                        crate::corelog::line(&format!("[core] summarization failed: {msg}"));
                        ctx.log.emit(json!({"kind":"compaction_failed","reason":reason,"error":msg,"fromExtension":false}));
                        fire_compact_failed(ctx, reason, Some(&msg), false);
                        return;
                    }
                }
            }
        };

        // Persist the upstream-shaped entry, then the events.
        let entry_id = ctx
            .session
            .append_compaction(&summary, &first_kept, tokens_before, usage.as_ref(), from_hook, Some(system_msg));
        let file_entry = ctx
            .session
            .entries()
            .into_iter()
            .find(|e| e.get("id").and_then(Value::as_str) == Some(entry_id.as_str()))
            .unwrap_or(Value::Null);
        ctx.log.emit(json!({"kind":"compaction","summary":summary,"firstKeptEntryId":first_kept,
            "tokensBefore":tokens_before,"fromExtension":from_hook,"reason":reason}));
        if subscribed(ctx, "session_compact") {
            ctx.client.fire(json!({"type":"session_compact","event":{"type":"session_compact",
                "compactionEntry":file_entry,"fromExtension":from_hook,"reason":reason,"willRetry":false}}));
        }
        crate::corelog::line(&format!(
            "[core] compacted: entry {entry_id} (tokens_before={tokens_before}, from_hook={from_hook})"
        ));
    }
}

/// Full transcript for `context_with_system`: leading system message (prompt +
/// tool declarations), custom messages from before_agent_start, then the
/// session transcript. Degraded: extension tools are dropped from the set.

/// Full transcript for `context_with_system`: leading system message (prompt +
/// tool declarations), custom messages from before_agent_start, then the
/// session transcript. Degraded: extension tools are dropped from the set.
/// Model tool set: built-ins merged with `ready.tools` via the upstream
/// precedence (extension tools shadow built-ins by name). Degraded: extension
/// tools are dropped from the set.
fn merged_tool_defs(ctx: &Core) -> Vec<Value> {
    let ext = if ctx.client.state() == State::Running {
        ctx.ext_tools.lock().unwrap().clone()
    } else {
        Vec::new()
    };
    tools::merge_with_extensions(tools::schemas(), ext)
}
fn build_conversation(ctx: &Core, system_prompt: &str, custom: &[Value]) -> Vec<Value> {
    let tool_defs = merged_tool_defs(ctx);
    let mut v = vec![json!({"role":"system","content":system_prompt,"toolsAdded":tool_defs,"timestamp":now_ms()})];
    v.extend(custom.iter().cloned());
    v.extend(ctx.session.transcript().iter().cloned());
    v
}

fn assistant_message(turn: &AssistantTurn, model: &Value) -> Value {
    let mut content = Vec::new();
    if let Some(t) = &turn.text {
        content.push(json!({"type":"text","text":t}));
    }
    for tc in &turn.tool_calls {
        // pi-ai shape: toolCall blocks carry `input` (upstream session files).
        content.push(json!({"type":"toolCall","id":tc.id,"name":tc.name,"input":tc.input}));
    }
    let (provider, mid) = (
        model.get("provider").cloned().unwrap_or(json!("fake")),
        model.get("id").cloned().unwrap_or(json!("fake")),
    );
    json!({"role":"assistant","content":content,"api":"fake","provider":provider,"model":mid,
        "usage":{"input":turn.usage.input,"output":turn.usage.output,
                 "cacheRead":turn.usage.cache_read,"cacheWrite":turn.usage.cache_write},
        "stopReason":if turn.tool_calls.is_empty(){"stop"}else{"toolUse"},"timestamp":crate::session::now_iso()})
}

/// WP-701: `ready`/`providers_changed` `providers` payload -> extension model
/// map keyed `provider/id` (per-turn routing + set_model validation key).
pub(crate) fn parse_providers(payload: &Value) -> std::collections::BTreeMap<String, Value> {
    let mut map = std::collections::BTreeMap::new();
    let Some(providers) = payload.get("providers").and_then(Value::as_array) else {
        return map;
    };
    for entry in providers {
        // Legacy name-only entries are not routable.
        let (Some(name), Some(models)) = (entry.get("name").and_then(Value::as_str), entry.get("models").and_then(Value::as_array)) else {
            continue;
        };
        for m in models {
            let Some(id) = m.get("id").and_then(Value::as_str) else { continue };
            let provider = m.get("provider").and_then(Value::as_str).unwrap_or(name);
            let mut model = m.clone();
            model["provider"] = Value::String(provider.to_string());
            map.insert(format!("{provider}/{id}"), model);
        }
    }
    map
}

/// WP-701: token usage from an upstream `done` message (camelCase wire).
fn usage_from_message(v: Option<&Value>) -> crate::model::Usage {
    let Some(u) = v else { return crate::model::Usage::default() };
    crate::model::Usage {
        input: u.get("input").and_then(Value::as_u64).unwrap_or(0),
        output: u.get("output").and_then(Value::as_u64).unwrap_or(0),
        cache_read: u.get("cacheRead").and_then(Value::as_u64).unwrap_or(0),
        cache_write: u.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// WP-701: maps a run of upstream `AssistantMessageEvent`s (forwarded by the
/// sidecar as `provider_stream_event` lines) to an AssistantTurn. `done` is
/// authoritative for text/tool calls/usage; accumulated `text_delta` /
/// `toolcall_end` state is the fallback when the done message carries no
/// content. `error` is terminal; a run without a terminal event is an error.
pub(crate) fn provider_events_to_turn(events: &[Value]) -> Result<AssistantTurn, String> {
    use crate::model::{ToolCallSpec, Usage};
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCallSpec> = Vec::new();
    for ev in events {
        match ev.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                if let Some(d) = ev.get("delta").and_then(Value::as_str) { text.push_str(d); }
            }
            Some("toolcall_end") => {
                if let Some(tc) = ev.get("toolCall") {
                    tool_calls.push(ToolCallSpec {
                        id: tc.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        name: tc.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        input: tc.get("arguments").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            Some("done") => {
                let message = ev.get("message").cloned().unwrap_or(Value::Null);
                let usage = usage_from_message(message.get("usage"));
                // The done message is authoritative when it carries content;
                // otherwise fall back to the accumulated deltas/tool calls.
                let content = message.get("content").and_then(Value::as_array);
                let has_content = content.is_some_and(|c| !c.is_empty());
                if has_content {
                    let mut final_text = String::new();
                    let mut final_tools: Vec<ToolCallSpec> = Vec::new();
                    for part in content.unwrap() {
                        match part.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(t) = part.get("text").and_then(Value::as_str) { final_text.push_str(t); }
                            }
                            Some("toolCall") => final_tools.push(ToolCallSpec {
                                id: part.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                                name: part.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                                input: part.get("arguments").cloned().unwrap_or(Value::Null),
                            }),
                            _ => {}
                        }
                    }
                    return Ok(AssistantTurn { text: Some(final_text), tool_calls: final_tools, usage });
                }
                return Ok(AssistantTurn { text: Some(text), tool_calls, usage });
            }
            Some("error") => {
                let msg = ev.pointer("/error/errorMessage").and_then(Value::as_str)
                    .unwrap_or("provider stream error");
                return Err(msg.to_string());
            }
            _ => {}
        }
    }
    Err("provider stream ended without a terminal event".into())
}

#[cfg(test)]
mod provider_stream_tests {
    use super::{parse_providers, provider_events_to_turn, usage_from_message};
    use crate::model::Usage;
    use serde_json::json;

    fn done_event(content: serde_json::Value, usage: Option<serde_json::Value>) -> serde_json::Value {
        let mut message = json!({"role":"assistant","content":content,"stopReason":"stop","timestamp":1});
        if let Some(u) = usage {
            message["usage"] = u;
        }
        json!({"type":"done","reason":"stop","message":message})
    }

    #[test]
    fn maps_text_deltas_and_toolcall_end_with_done_message() {
        let events = vec![
            json!({"type":"start","partial":{"role":"assistant","content":[]}}),
            json!({"type":"text_delta","contentIndex":0,"delta":"Hello ","partial":{}}),
            json!({"type":"text_delta","contentIndex":0,"delta":"world","partial":{}}),
            json!({"type":"toolcall_end","contentIndex":1,"toolCall":{"type":"toolCall","id":"tc_1","name":"bash","arguments":{"command":"ls"}},"partial":{}}),
            done_event(
                json!([{"type":"text","text":"Hello world"},{"type":"toolCall","id":"tc_1","name":"bash","arguments":{"command":"ls"}}]),
                Some(json!({"input":7,"output":11,"cacheRead":3,"cacheWrite":4})),
            ),
        ];
        let turn = provider_events_to_turn(&events).expect("done stream maps to a turn");
        assert_eq!(turn.text.as_deref(), Some("Hello world"));
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "tc_1");
        assert_eq!(turn.tool_calls[0].name, "bash");
        assert_eq!(turn.tool_calls[0].input, json!({"command":"ls"}));
        assert_eq!(turn.usage, Usage { input: 7, output: 11, cache_read: 3, cache_write: 4 });
    }

    #[test]
    fn terminal_error_event_yields_the_error_message() {
        let events = vec![json!({"type":"start","partial":{}}),
            json!({"type":"error","reason":"error","error":{"role":"assistant","stopReason":"error","errorMessage":"boom from fake provider"}})];
        let err = provider_events_to_turn(&events).unwrap_err();
        assert_eq!(err, "boom from fake provider");
    }

    #[test]
    fn stream_without_terminal_event_is_an_error() {
        let events = vec![json!({"type":"start","partial":{}}), json!({"type":"text_delta","contentIndex":0,"delta":"x","partial":{}})];
        let err = provider_events_to_turn(&events).unwrap_err();
        assert!(err.contains("terminal"), "{err}");
    }

    #[test]
    fn done_without_message_falls_back_to_accumulated_state() {
        let events = vec![
            json!({"type":"text_delta","contentIndex":0,"delta":"acc ","partial":{}}),
            json!({"type":"text_delta","contentIndex":0,"delta":"text","partial":{}}),
            json!({"type":"toolcall_end","contentIndex":1,"toolCall":{"type":"toolCall","id":"tc_9","name":"read","arguments":{"path":"f"}},"partial":{}}),
            json!({"type":"done","reason":"stop","message":{"role":"assistant","content":[]}}),
        ];
        let turn = provider_events_to_turn(&events).expect("fallback turn");
        assert_eq!(turn.text.as_deref(), Some("acc text"));
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read");
        assert_eq!(turn.usage, Usage::default());
    }

    #[test]
    fn parse_providers_builds_provider_id_keys() {
        let payload = json!({"providers":[
            {"name":"faketest","models":[
                {"provider":"faketest","id":"faketest/echo-1","api":"faketest-api","name":"Fake Echo 1"}
            ]},
            {"name":"other","models":[
                {"id":"other/m-1"}
            ]}
        ]});
        let map = parse_providers(&payload);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("faketest/faketest/echo-1").expect("first model").get("api"), Some(&json!("faketest-api")));
        // A model entry without `provider` inherits the registered name.
        let m = map.get("other/other/m-1").expect("second model");
        assert_eq!(m.get("provider"), Some(&json!("other")));
        // Legacy name-only entries are not routable.
        let map = parse_providers(&json!({"providers":["legacy-name"]}));
        assert!(map.is_empty());
    }

    #[test]
    fn usage_from_message_maps_camel_case_fields() {
        let u = usage_from_message(Some(&json!({"input":1,"output":2,"cacheRead":3,"cacheWrite":4,"totalTokens":10})));
        assert_eq!(u, Usage { input: 1, output: 2, cache_read: 3, cache_write: 4 });
        assert_eq!(usage_from_message(None), Usage::default());
    }
}

fn summarize(content: &Value) -> String {
    let s = content.as_array()
        .map(|a| a.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let s = s.trim().to_string();
    if s.chars().count() > 160 {
        let mut out: String = s.chars().take(160).collect();
        out.push('…');
        out
    } else {
        s
    }
}
