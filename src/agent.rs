use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use crate::client::{BridgeClient, Decision, State};
use crate::model::{AssistantTurn, FakeDriver, ModelDriver};
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
}

pub enum Item {
    User(QueuedUser),
    End,
}

type Ack = Option<tokio::sync::oneshot::Sender<()>>;

#[derive(Clone)]
pub struct EventLog {
    tx: Option<mpsc::UnboundedSender<(Value, Ack)>>,
    sub: tokio::sync::broadcast::Sender<Value>,
}

impl EventLog {
    /// `headless` = NDJSON on stdout (the core output contract for
    /// --scenario/--prompt/--list-sessions runs); the TUI renders in-process
    /// instead and passes `false`.
    pub fn start(headless: bool) -> Self {
        let (sub, _) = tokio::sync::broadcast::channel(8192); // 8192: streaming bursts (WP-603)
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
        Self { tx, sub }
    }

    /// Subscribe to the in-process event stream (TUI).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Value> {
        self.sub.subscribe()
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
pub struct Core {
    pub client: Arc<BridgeClient>,
    pub session: Arc<Session>,
    pub events: Arc<Mutex<HashSet<String>>>,            // ready.events (subscribed event names)
    pub ext_tools: Arc<Mutex<Vec<Value>>>,              // ready.tools entries (pass-through schemas)
    pub ready: Mutex<Option<tokio::sync::oneshot::Sender<Value>>>, // full `ready` payload → cli
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
    pub cost_total: Arc<Mutex<f64>>,
    /// TUI mode (WP-604): blocking ui dialogs are forwarded to the TUI
    /// instead of answering with the upstream defaults.
    pub tui: bool,
    /// Dialog requests for the TUI (WP-604); headless keeps it unconnected.
    pub ui_tx: mpsc::UnboundedSender<Value>,
}

/// True when the core may send the event: it must appear in `ready.events` and
/// in the core catalog. `context_with_system` doubles as the two-phase context
/// hook — the single upstream emitContext runs there (sidecar resolution 4), so
/// a subscription to `context` also routes through it.
fn subscribed(ctx: &Core, name: &str) -> bool {
    let ev = ctx.events.lock().unwrap();
    ev.contains(name) || (name == "context_with_system" && ev.contains("context"))
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
pub async fn event_task(mut events: mpsc::UnboundedReceiver<Value>, ctx: Arc<Core>) {
    while let Some(ev) = events.recv().await {
        match ev.get("type").and_then(Value::as_str).unwrap_or("") {
            "ready" => {
                let empty = Vec::new();
                let names: Vec<String> = ev.get("events").and_then(Value::as_array).unwrap_or(&empty)
                    .iter().filter_map(Value::as_str).map(str::to_string).collect();
                ctx.events.lock().unwrap().extend(names.iter().cloned());
                ctx.ext_tools.lock().unwrap().clone_from(ev.get("tools").and_then(Value::as_array).unwrap_or(&empty));
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
                let _ = ctx.msg_tx.send(Item::User(QueuedUser { text, source: "extension", steps: Vec::new(), images: Vec::new() }));
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
                        if crate::registry::ModelRegistry::bundled().resolve(&p, &m).is_none() {
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
    let data = match execute_tool_call(ctx, &call_id, &name, &input).await {
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
async fn execute_tool_call(ctx: &Core, call_id: &str, name: &str, input: &Value) -> Result<(Value, bool), String> {
    let event = json!({"type":"tool_call","toolCallId":call_id,"toolName":name,"input":input});
    // Degraded default: no block, input unchanged.
    let data = decision_data(ctx, ctx.client.request("tool_call", json!({"event":event})).await, "tool_call", &event, &json!({"result":Value::Null,"input":input}));
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
    if tools::is_builtin(name) {
        return Ok(tools::execute(name, input).await);
    }
    let known = ctx.ext_tools.lock().unwrap().iter().any(|t| t.get("name").and_then(Value::as_str) == Some(name));
    if !known {
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
fn delta_emitter<'c>(ctx: &'c Core, turn_index: u32) -> Box<dyn FnMut(&str) + Send + 'c> {
    Box::new(move |chunk: &str| {
        ctx.log
            .emit_tui(json!({"kind":"model_delta","turnIndex":turn_index,"text":chunk}));
    })
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
                if let Err(msg) = run_agent(&ctx, &u).await {
                    crate::corelog::line(&format!("[core] agent run failed: {msg}"));
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
                Some(Box::new(FakeDriver::echo_mode()))
            } else {
                Some(Box::new(FakeDriver::from_steps(&u.steps).map_err(|e| e.to_string())?))
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

    // `before_agent_start`: prepend returned custom messages; a returned
    // systemPrompt (present only when a handler changed it) replaces the prompt
    // for this run.
    let mut custom: Vec<Value> = Vec::new();
    let mut system_prompt = BASE_SYSTEM_PROMPT.to_string();
    if subscribed(ctx, "before_agent_start") {
        let event = json!({"type":"before_agent_start","messageType":"user","prompt":text});
        // BuildSystemPromptOptions subset for the sidecar: core cwd, currently
        // active built-in tool names, empty sections (handlers mutate in place).
        let selected_tools: Vec<String> = tools::schemas().iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        let spon = json!({
            "cwd": std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            "selectedTools": selected_tools,
            "sections": {},
        });
        let req = json!({"event":event,"systemPrompt":BASE_SYSTEM_PROMPT,"systemPromptOptions":spon});
        let data = decision_data(ctx, ctx.client.request("before_agent_start", req.clone()).await, "before_agent_start", &req, &Value::Null);
        if let Some(ms) = data.get("messages").and_then(Value::as_array) {
            custom = ms.to_vec();
        }
        if let Some(p) = data.get("systemPrompt").and_then(Value::as_str) {
            system_prompt = p.to_string();
        }
    }

    if subscribed(ctx, "agent_start") { ctx.client.fire(json!({"type":"agent_start","event":{"type":"agent_start"}})); }
    ctx.log.emit(json!({"kind":"agent_start"}));
    let run_start = ctx.session.transcript().len();

    let mut turn_index = 0u32;
    loop {
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

        // WP-603: streamed text chunks become TUI-only `model_delta` events
        // (the headless NDJSON contract keeps its single `model_output`).
        let turn = match fake_driver.as_ref() {
            Some(d) => d.complete(&convo, delta_emitter(ctx, turn_index)).await,
            None => {
                let drv = ctx.provider.lock().unwrap().clone();
                match drv.as_ref() {
                    Some(ProviderDriver::OpenAi(d)) => d.complete(&convo, delta_emitter(ctx, turn_index)).await,
                    Some(ProviderDriver::Anthropic(d)) => d.complete(&convo, delta_emitter(ctx, turn_index)).await,
                    Some(ProviderDriver::Google(d)) => d.complete(&convo, delta_emitter(ctx, turn_index)).await,
                    Some(ProviderDriver::Bedrock(d)) => d.complete(&convo, delta_emitter(ctx, turn_index)).await,
                    None => unreachable!("fake_driver is None only while a provider exists"),
                }
            }
        }
        .map_err(|e| e.to_string())?;

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
        if subscribed(ctx, "turn_start") { ctx.client.fire(json!({"type":"turn_start","event":{"type":"turn_start","turnIndex":turn_index,"timestamp":now_ms()}})); }
        ctx.log.emit(json!({"kind":"turn_start","turnIndex":turn_index}));

        // The fake model does not stream: message_start + message_end only.
        let assistant = assistant_message(&turn);
        if subscribed(ctx, "message_start") { ctx.client.fire(json!({"type":"message_start","event":{"type":"message_start","message":assistant}})); }
        if subscribed(ctx, "message_end") { ctx.client.fire(json!({"type":"message_end","event":{"type":"message_end","message":assistant}})); }
        let msg_id = ctx.session.append_message(&assistant);
        ctx.log.emit(json!({"kind":"model_output","turnIndex":turn_index,"text":turn.text.as_deref(),
            "toolCalls":turn.tool_calls.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()}));

        let mut tool_results: Vec<Value> = Vec::new();
        let mut tool_result_ids: Vec<String> = Vec::new();
        let mut all_terminate = true;
        for tc in &turn.tool_calls {
            match execute_tool_call(ctx, &tc.id, &tc.name, &tc.input).await {
                Ok((trm, terminate)) => {
                    tool_result_ids.push(ctx.session.append_message(&trm));
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

        // A turn with no tool calls ends the run; `terminate` is honored only
        // when EVERY finalized result in the batch carried it (upstream rule).
        if turn.tool_calls.is_empty() || (all_terminate && !tool_results.is_empty()) {
            break;
        }
        turn_index += 1;
    }

    let run_messages = ctx.session.transcript()[run_start..].to_vec();
    if subscribed(ctx, "agent_end") { ctx.client.fire(json!({"type":"agent_end","event":{"type":"agent_end","messages":run_messages}})); }
    ctx.log.emit(json!({"kind":"agent_end"}));
    Ok(())
}

/// Full transcript for `context_with_system`: leading system message (prompt +
/// tool declarations), custom messages from before_agent_start, then the
/// session transcript. Degraded: extension tools are dropped from the set.
fn build_conversation(ctx: &Core, system_prompt: &str, custom: &[Value]) -> Vec<Value> {
    let mut tool_defs = tools::schemas();
    if ctx.client.state() == State::Running {
        tool_defs.extend(ctx.ext_tools.lock().unwrap().iter().cloned()); // ready.tools pass-through
    }
    let mut v = vec![json!({"role":"system","content":system_prompt,"toolsAdded":tool_defs,"timestamp":now_ms()})];
    v.extend(custom.iter().cloned());
    v.extend(ctx.session.transcript().iter().cloned());
    v
}

fn assistant_message(turn: &AssistantTurn) -> Value {
    let mut content = Vec::new();
    if let Some(t) = &turn.text {
        content.push(json!({"type":"text","text":t}));
    }
    for tc in &turn.tool_calls {
        // pi-ai shape: toolCall blocks carry `input` (upstream session files).
        content.push(json!({"type":"toolCall","id":tc.id,"name":tc.name,"input":tc.input}));
    }
    json!({"role":"assistant","content":content,"api":"fake","provider":"fake","model":"fake",
        "usage":{"input":turn.usage.input,"output":turn.usage.output,
                 "cacheRead":turn.usage.cache_read,"cacheWrite":turn.usage.cache_write},
        "stopReason":if turn.tool_calls.is_empty(){"stop"}else{"toolUse"},"timestamp":crate::session::now_iso()})
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
