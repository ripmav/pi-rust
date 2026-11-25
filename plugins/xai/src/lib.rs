//! xAI (Grok) provider plugin — port of the upstream `xaiProvider`
//! (pinned upstream 64a15f4) for the pi provider plugin ABI v1
//! (`docs/provider-plugin-abi.md`).
//!
//! **Wire API** (`openai-responses`, upstream `api/openai-responses.ts`):
//! `POST {base}/responses` (`stream: true`, `store: false`), SSE events —
//! `output_item.added/done`, `output_text.delta`, `refusal.delta`,
//! `function_call_arguments.delta/done`, `completed/incomplete/failed`.
//! Base URL: `XAI_BASE_URL` (mock hook) else `https://api.x.ai/v1`.
//!
//! **OAuth** (RFC 8628 device code, upstream `auth/oauth/xai.ts`):
//! `POST {host}/oauth2/device/code` + `POST {host}/oauth2/token`; client id
//! `b1a00492-073a-47ea-816f-4c329264a828`, scope
//! `openid profile email offline_access grok-cli:access api:access`,
//! `referrer: pi`. Host: `XAI_OAUTH_HOST` (mock hook) else
//! `https://auth.x.ai`. The verification URI is validated as https-only
//! (upstream rule — a malicious response must not open something else).
//! The credential the core stores (upstream `OAuthCredential` shape):
//! `{type: "oauth", access, refresh, expires}` — `expires` is the RAW
//! `now + expires_in*1000` (upstream bakes a 5-minute skew at store time;
//! the core's 5-minute validity margin applies the same rule uniformly —
//! one place, E3S4 semantics). `refresh_token` may be omitted by the
//! server on refresh (upstream: keep the previous one).
//!
//! **Refresh** (E4S2 core-side refresh, upstream `refreshXaiToken`): the
//! core sends `auth_start` the request
//! `{"action":"refresh","credential":{...}}`; this plugin answers
//! `done` (rotated credential) or `failed` synchronously. The core writes
//! the rotation back to auth.json — the plugin never touches the store.
//!
//! **v0 limits** (documented port deviations): the stream request carries
//! no temperature/reasoning/toolChoice/session (the ABI v0 request shape),
//! so reasoning `effort`/`include` and prompt-cache fields are not sent
//! (provider defaults apply); tool calls ride the conversation (the
//! system message's `toolsAdded`) as `tools`.

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

const ABI_VERSION: u32 = 1;

// Upstream constants (pinned 64a15f4, auth/oauth/xai.ts).
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEFAULT_OAUTH_HOST: &str = "https://auth.x.ai";
const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
/// The xAI device code allows `interval: 0`; fall back to the poller's
/// default instead of failing on non-positive values (upstream rule).
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const DEFAULT_TOKEN_LIFETIME_SECONDS: u64 = 3600;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry-shaped catalog (pinned upstream data/xai.json, all
/// `openai-responses` chat models — the ids the sidecar/upstream advertise).
const CATALOG: &str = include_str!("catalog.json");

// ---------------------------------------------------------------------------
// C ABI vtable (docs/provider-plugin-abi.md — ABI v1)

#[repr(C)]
pub struct PiProviderVtable {
    pub abi_version: extern "C" fn() -> u32,
    pub name: extern "C" fn() -> *const u8,
    pub list_models: extern "C" fn() -> *const u8,
    pub stream_open: extern "C" fn(*const u8) -> *const u8,
    pub stream_next: extern "C" fn(*const u8) -> *const u8,
    pub stream_close: extern "C" fn(*const u8),
    pub auth_start: extern "C" fn(*const u8) -> *const u8,
    pub auth_status: extern "C" fn(*const u8) -> *const u8,
    pub dispose: extern "C" fn(),
}

static VT: PiProviderVtable = PiProviderVtable {
    abi_version,
    name,
    list_models,
    stream_open,
    stream_next,
    stream_close,
    auth_start,
    auth_status,
    dispose,
};

/// The entry symbol the core's loader dlsym's.
#[no_mangle]
pub extern "C" fn pi_provider_plugin_init() -> *const PiProviderVtable {
    &VT
}

// ---------------------------------------------------------------------------
// Boundary helpers (the core copies every returned string before the next
// vtable call; one buffer per calling thread keeps the plugin leak-free).

thread_local! {
    static OUT_BUF: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn out(s: &str) -> *const u8 {
    OUT_BUF.with(|b| {
        let mut b = b.borrow_mut();
        b.clear();
        b.extend_from_slice(s.as_bytes());
        b.push(0);
        b.as_ptr()
    })
}

fn read_cstr(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: every pointer the core passes is a valid NUL-terminated UTF-8
    // string it owns for the duration of the call. The `as *const _`
    // reinterprets to `*const c_char` (1-byte type, platform-dependent).
    unsafe { CStr::from_ptr(p as *const _) }
        .to_str()
        .unwrap_or("")
        .to_string()
}

fn err(s: &str) -> *const u8 {
    out(&format!("{{\"error\":{}}}", json!(s)))
}

// ---------------------------------------------------------------------------
// Vtable methods

extern "C" fn abi_version() -> u32 {
    ABI_VERSION
}

static NAME_BYTES: &[u8] = b"xai\0";

extern "C" fn name() -> *const u8 {
    NAME_BYTES.as_ptr()
}

extern "C" fn list_models() -> *const u8 {
    // `provider` is informational (the core sets it from the vtable name);
    // the entries are registry-shaped (camelCase, the core's Model schema).
    out(CATALOG)
}

// ---------------------------------------------------------------------------
// Streaming (the vtable calls block per event; the wire task runs on the
// plugin's own runtime)

struct StreamState {
    rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
    terminal: bool,
}

static STREAMS: LazyLock<Mutex<HashMap<String, StreamState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("xai plugin: tokio runtime")
    })
}

/// `{"handle": ...}` on success, `{"error": ...}` on a request-level failure
/// (wire failures surface as stream events, not open failures).
extern "C" fn stream_open(request: *const u8) -> *const u8 {
    let req: Value = match serde_json::from_str(&read_cstr(request)) {
        Ok(v) => v,
        Err(e) => return err(&format!("stream request is not valid JSON: {e}")),
    };
    // The core sends the full registry model (id/name/api/reasoning/...);
    // `reasoning` drives the upstream instruction role (developer vs system).
    let model = req.get("model").cloned().unwrap_or(Value::Null);
    let model_id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reasoning = model
        .get("reasoning")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let messages = req.get("messages").cloned().unwrap_or(Value::Array(vec![]));
    let base = std::env::var("XAI_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string();
    // The core resolved the credential (auth.json / login / refresh); the
    // plugin only consumes it here — it is never stored plugin-side.
    let token = req
        .get("credential")
        .and_then(|c| c.get("key").or_else(|| c.get("access")))
        .and_then(Value::as_str)
        .map(str::to_string);

    let handle = format!("h{}", NEXT_HANDLE.fetch_add(1, Ordering::SeqCst) + 1);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    STREAMS.lock().unwrap().insert(
        handle.clone(),
        StreamState {
            rx,
            terminal: false,
        },
    );

    runtime().spawn({
        let tx_outer = tx.clone();
        let inner = runtime().spawn(async move {
            stream_responses(&base, &model_id, reasoning, &messages, token.as_deref(), tx).await;
        });
        async move {
            // A panicking producer must not kill the stream silently: surface
            // it as a typed `error` event (the ABI's error convention).
            if let Err(e) = inner.await {
                let msg = if e.is_panic() {
                    format!("the plugin's stream task panicked: {e}")
                } else {
                    format!("the plugin's stream task failed: {e}")
                };
                let _ = tx_outer.send(json!({"type": "error", "error": msg}));
            }
        }
    });
    out(&json!({ "handle": handle }).to_string())
}

/// The next stream event JSON, or NULL when the stream has ended (after the
/// `done`/`error` event).
extern "C" fn stream_next(handle: *const u8) -> *const u8 {
    let h = read_cstr(handle);
    // Take the state out while the (blocking) recv runs: no other thread can
    // steal a message from a second receiver, and the lock is never held
    // across the block.
    let Some(mut s) = STREAMS.lock().unwrap().remove(&h) else {
        return std::ptr::null();
    };
    if s.terminal {
        STREAMS.lock().unwrap().insert(h, s);
        return std::ptr::null();
    }
    let ev = match runtime().block_on(s.rx.recv()) {
        Some(v) => v,
        None => {
            // The producer exited without a final event (a cancelled task).
            s.terminal = true;
            STREAMS.lock().unwrap().insert(h, s);
            return std::ptr::null();
        }
    };
    if matches!(
        ev.get("type").and_then(Value::as_str),
        Some("done") | Some("error")
    ) {
        s.terminal = true;
    }
    STREAMS.lock().unwrap().insert(h, s);
    out(&ev.to_string())
}

/// Frees the plugin-side stream state; an aborted HTTP stream is dropped
/// with it. Safe to call after the stream already ended.
extern "C" fn stream_close(handle: *const u8) {
    let h = read_cstr(handle);
    STREAMS.lock().unwrap().remove(&h);
}

// ---------------------------------------------------------------------------
// The openai-responses wire (upstream `api/openai-responses.ts` +
// `openai-responses-shared.ts`: message conversion, `buildParams` subset,
// `processResponsesStream` event handling)

/// Upstream `convertResponsesMessages` subset: the pi conversation →
/// Responses `input` items. v0: system → one `developer` item (upstream
/// role for reasoning models; `system` otherwise), user text/image,
/// assistant text + `function_call`, `toolResult` → `function_call_output`.
/// (tool_search/configuration_update items and foreign-tool-call id
/// rewriting are not ported — v0.)
fn convert_responses_messages(model_reasoning: bool, convo: &[Value]) -> Vec<Value> {
    let instruction_role = if model_reasoning {
        "developer"
    } else {
        "system"
    };
    let mut out = Vec::new();
    for m in convo {
        match m.get("role").and_then(Value::as_str) {
            Some("system") => {
                if let Some(c) = m.get("content").and_then(Value::as_str) {
                    if !c.is_empty() {
                        out.push(json!({ "role": instruction_role, "content": c }));
                    }
                }
            }
            Some("user") => match m.get("content") {
                Some(Value::String(s)) => out.push(json!({
                    "role": "user",
                    "content": [{ "type": "input_text", "text": s }],
                })),
                Some(Value::Array(blocks)) => {
                    let content: Vec<Value> = blocks
                        .iter()
                        .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                            Some("text") => b
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|t| json!({ "type": "input_text", "text": t })),
                            Some("image") => {
                                let mime = b
                                    .get("mimeType")
                                    .and_then(Value::as_str)
                                    .unwrap_or("image/png");
                                let data = b.get("data")?.as_str()?;
                                Some(json!({
                                    "type": "input_image",
                                    "detail": "auto",
                                    "image_url": format!("data:{mime};base64,{data}"),
                                }))
                            }
                            _ => None,
                        })
                        .collect();
                    if !content.is_empty() {
                        out.push(json!({ "role": "user", "content": content }));
                    }
                }
                _ => {}
            },
            Some("assistant") => {
                if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(Value::as_str) {
                                    if !t.is_empty() {
                                        out.push(json!({
                                            "type": "message",
                                            "role": "assistant",
                                            "content": [{ "type": "output_text", "text": t }],
                                        }));
                                    }
                                }
                            }
                            // Upstream id: `${call_id}|${item_id}` (same
                            // provider replay keeps both halves).
                            Some("toolCall") => {
                                let id = b.get("id").and_then(Value::as_str).unwrap_or("");
                                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                                let args = b
                                    .get("input")
                                    .or_else(|| b.get("arguments"))
                                    .cloned()
                                    .unwrap_or_else(|| json!({}));
                                out.push(json!({
                                    "type": "function_call",
                                    "call_id": id,
                                    "id": id,
                                    "name": name,
                                    "arguments": args.to_string(),
                                }));
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("toolResult") => {
                let text = text_of(m.get("content"));
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
                    "output": text,
                }));
            }
            _ => {}
        }
    }
    out
}

/// The system message's `toolsAdded` (the core's tools channel) → Responses
/// `tools` (upstream `convertResponsesTools` subset: name/description/
/// parameters, strict mode off in v0).
fn responses_tools(convo: &[Value]) -> Vec<Value> {
    let defs = convo
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
        .and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    defs.iter()
        .filter_map(|d| {
            let name = d.get("name").and_then(Value::as_str)?;
            Some(json!({
                "type": "function",
                "name": name,
                "description": d.get("description").cloned().unwrap_or(Value::Null),
                "parameters": d.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
            }))
        })
        .collect()
}

/// Joins the text of a pi content value (string or text blocks).
fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// The `openai-responses` stream: `POST {base}/responses`, SSE. Emits
/// `delta`/`done`/`error` events on `tx` (exactly one terminal).
async fn stream_responses(
    base: &str,
    model_id: &str,
    reasoning: bool,
    messages: &Value,
    token: Option<&str>,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let fail = |msg: String| {
        let _ = tx.send(json!({"type": "error", "error": msg}));
    };
    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return fail(format!("http client: {e}")),
    };
    let convo: Vec<Value> = messages.as_array().cloned().unwrap_or_default();
    let tools = responses_tools(&convo);
    let mut body = json!({
        "model": model_id,
        "input": convert_responses_messages(reasoning, &convo),
        "stream": true,
        "store": false,
    });
    if !tools.is_empty() {
        // Upstream `buildParams`: tools ride the request (v0: from the
        // conversation's system `toolsAdded`; no toolChoice in the ABI v0).
        body["tools"] = json!(tools);
    }
    let mut reqb = client.post(format!("{base}/responses")).json(&body);
    if let Some(t) = token {
        reqb = reqb.bearer_auth(t);
    }
    let resp = match reqb.send().await {
        Ok(r) => r,
        Err(e) => return fail(format!("request failed: {e}")),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return fail(format!(
            "HTTP {status}: {}",
            body.chars().take(160).collect::<String>()
        ));
    }
    // Stream slots: output_index -> kind (upstream `outputSlots`).
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Slot {
        Text,
        ToolCall,
    }
    let mut slots: HashMap<u64, Slot> = HashMap::new();
    let mut text_full = String::new();
    // tool call: output_index -> (id, name, raw-arguments accumulator)
    let mut tools: HashMap<u64, (String, String, String, bool)> = HashMap::new();
    let mut usage = json!({"input": 0, "output": 0, "cache_read": 0, "cache_write": 0});
    let mut saw_terminal = false;

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return fail(format!("stream error: {e}")),
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim_end().to_string();
            buf.drain(..nl + 1);
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let etype = ev.get("type").and_then(Value::as_str).unwrap_or_default();
            match etype {
                "response.output_item.added" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    match ev
                        .pointer("/item/type")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                    {
                        "message" => {
                            slots.insert(idx, Slot::Text);
                        }
                        "function_call" | "custom_tool_call" => {
                            let item = &ev["item"];
                            let id = item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let args = item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            slots.insert(idx, Slot::ToolCall);
                            tools.insert(
                                idx,
                                (
                                    if item_id.is_empty() {
                                        id.clone()
                                    } else {
                                        format!("{id}|{item_id}")
                                    },
                                    name,
                                    args,
                                    false,
                                ),
                            );
                        }
                        _ => {}
                    }
                }
                // Reasoning items are not ported (v0: no thinking support in
                // the ABI); their deltas carry nothing to the turn.
                "response.output_text.delta" | "response.refusal.delta" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    if slots.get(&idx) == Some(&Slot::Text) {
                        let d = ev.get("delta").and_then(Value::as_str).unwrap_or_default();
                        text_full.push_str(d);
                        let _ = tx.send(json!({"type": "delta", "delta": d}));
                    }
                }
                "response.function_call_arguments.delta" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(t) = tools.get_mut(&idx) {
                        t.2.push_str(ev.get("delta").and_then(Value::as_str).unwrap_or_default());
                    }
                }
                "response.function_call_arguments.done" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(t) = tools.get_mut(&idx) {
                        if let Some(a) = ev.get("arguments").and_then(Value::as_str) {
                            t.2 = a.to_string();
                        }
                        t.3 = true;
                    }
                }
                "response.output_item.done" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    let item_type = ev
                        .pointer("/item/type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if item_type == "message" {
                        // The item's content is authoritative (upstream).
                        let content = ev
                            .pointer("/item/content")
                            .and_then(Value::as_array)
                            .map(|c| {
                                c.iter()
                                    .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                                        Some("output_text") => {
                                            p.get("text").and_then(Value::as_str)
                                        }
                                        Some("refusal") => p.get("refusal").and_then(Value::as_str),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default();
                        if !content.is_empty() {
                            text_full = content;
                        }
                    } else if item_type == "function_call" {
                        if let Some(t) = tools.get_mut(&idx) {
                            if let Some(a) = ev.pointer("/item/arguments").and_then(Value::as_str) {
                                t.2 = a.to_string();
                            }
                            t.3 = true;
                        }
                    }
                }
                "response.completed" | "response.incomplete" => {
                    saw_terminal = true;
                    let r = &ev["response"];
                    if etype == "response.completed" {
                        // `response.incomplete` carries no usage; keep what
                        // we have (upstream: incomplete without
                        // max_output_tokens errors out below).
                    }
                    if let Some(u) = r.get("usage") {
                        let cached = u
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        let cache_write = u
                            .pointer("/input_tokens_details/cache_write_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        // OpenAI includes cached and cache-write tokens in
                        // input_tokens, so subtract both (upstream rule).
                        usage["input"] = json!(u
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                            .saturating_sub(cached + cache_write));
                        usage["output"] =
                            json!(u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0));
                        usage["cache_read"] = json!(cached);
                        usage["cache_write"] = json!(cache_write);
                    }
                    let status = r.get("status").and_then(Value::as_str).unwrap_or_default();
                    if status == "failed" || status == "cancelled" {
                        return fail(format!(
                            "OpenAI Responses stream ended with status {status}"
                        ));
                    }
                    if status == "incomplete" {
                        let reason = r
                            .pointer("/incomplete_details/reason")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if reason != "max_output_tokens" {
                            return fail(format!(
                                "Response incomplete: {}",
                                if reason.is_empty() {
                                    "no provider reason".to_string()
                                } else {
                                    reason.to_string()
                                }
                            ));
                        }
                    }
                }
                "response.failed" => {
                    let code = ev
                        .pointer("/response/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let msg = ev
                        .pointer("/response/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("no message");
                    return fail(format!("OpenAI Responses error {code}: {msg}"));
                }
                "error" => {
                    let code = ev.get("code").and_then(Value::as_str).unwrap_or("");
                    let msg = ev
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown error");
                    return fail(format!("OpenAI Responses error {code}: {msg}"));
                }
                _ => {} // response.created / unknown events: ignore (upstream)
            }
        }
    }
    if !saw_terminal {
        return fail("OpenAI Responses stream ended before a terminal response event".to_string());
    }
    // Tool calls whose `output_item.done` never arrived may have cut-off
    // arguments (upstream guard) — drop them rather than hand over garbage.
    let tool_calls: Vec<Value> = tools
        .values()
        .filter(|(_, _, _, done)| *done)
        .map(|(id, name, args, _)| {
            let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
            json!({ "id": id, "name": name, "input": input })
        })
        .collect();
    let text = if text_full.is_empty() {
        Value::Null
    } else {
        Value::String(text_full)
    };
    let _ = tx.send(json!({
        "type": "done",
        "turn": { "text": text, "tool_calls": tool_calls, "usage": usage },
    }));
}

// ---------------------------------------------------------------------------
// OAuth: device-code login + refresh (upstream `auth/oauth/xai.ts`)

struct AuthState {
    url: Option<String>,
    code: Option<String>,
    credential: Option<Value>,
    failed: Option<String>,
}

static AUTH: Mutex<AuthState> = Mutex::new(AuthState {
    url: None,
    code: None,
    credential: None,
    failed: None,
});

fn oauth_host() -> String {
    std::env::var("XAI_OAUTH_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Upstream `validateVerificationUri`: https-only (the URI is opened in the
/// user's browser; a malicious response must not launch something else).
fn validate_verification_uri(raw: &str) -> Result<String, String> {
    let rest = raw
        .strip_prefix("https://")
        .ok_or_else(|| "Untrusted verification URI in xAI OAuth response".to_string())?;
    if rest.is_empty() {
        return Err("Untrusted verification URI in xAI OAuth response".to_string());
    }
    Ok(raw.to_string())
}

async fn post_form(
    host: &str,
    path: &str,
    fields: &[(&str, &str)],
) -> Result<(u16, Value), String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .post(format!("{host}{path}"))
        .form(fields)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("xAI OAuth request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let body = if body.is_object() { body } else { json!({}) };
    Ok((status, body))
}

/// `credentialsFromTokenResponse`: access required, refresh may be omitted
/// on refresh (keep the previous), expires_in defaults to the 1h lifetime.
fn credentials_from_token_response(
    body: &Value,
    previous_refresh: Option<&str>,
) -> Result<Value, String> {
    let access = body
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid xAI OAuth response field: access_token".to_string())?;
    let refresh = match body.get("refresh_token").and_then(Value::as_str) {
        Some(r) if !r.is_empty() => r.to_string(),
        _ => previous_refresh
            .map(str::to_string)
            .ok_or_else(|| "Invalid xAI OAuth response field: refresh_token".to_string())?,
    };
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_TOKEN_LIFETIME_SECONDS);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    Ok(json!({
        "type": "oauth",
        "access": access,
        "refresh": refresh,
        "expires": now_ms + expires_in as i64 * 1000,
    }))
}

async fn device_authorization(host: &str) -> Result<(Value, Option<String>), String> {
    let (status, body) = post_form(
        host,
        "/oauth2/device/code",
        &[
            ("client_id", XAI_CLIENT_ID),
            ("scope", XAI_SCOPE),
            ("referrer", "pi"),
        ],
    )
    .await
    .map_err(|e| format!("xAI OAuth device authorization failed: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(request_failure("device authorization", status, &body));
    }
    let device_code = body
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid xAI OAuth response field: device_code".to_string())?;
    // The user code is surfaced in the pending status (`code` field — the
    // ABI v1 pending payload extension, E4S2); the core prints it.
    let user_code = body
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid xAI OAuth response field: user_code".to_string())?;
    let verification_uri = validate_verification_uri(
        body.get("verification_uri")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Invalid xAI OAuth response field: verification_uri".to_string())?,
    )?;
    let verification_uri_complete = body
        .get("verification_uri_complete")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(validate_verification_uri)
        .transpose()?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or_else(|| "Invalid xAI OAuth response field: expires_in".to_string())?;
    // RFC 8628 allows interval 0 (no minimum wait); fall back to the
    // poller's default (upstream rule).
    let interval = body
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
    Ok((
        json!({
            "device_code": device_code,
            "user_code": user_code,
            "expires_in": expires_in,
            "interval": interval,
        }),
        verification_uri_complete.or(Some(verification_uri)),
    ))
}

fn request_failure(action: &str, status: u16, body: &Value) -> String {
    let error = body.get("error").and_then(Value::as_str);
    let description = body
        .get("error_description")
        .and_then(Value::as_str)
        .map(|d| format!(": {d}"));
    let detail: Vec<String> = [error.map(|s| s.to_string()), description]
        .into_iter()
        .flatten()
        .collect();
    if detail.is_empty() {
        format!("xAI OAuth {action} failed (HTTP {status})")
    } else {
        format!(
            "xAI OAuth {action} failed (HTTP {status}): {}",
            detail.join(" ")
        )
    }
}

/// The device-code poll loop (upstream `pollForTokens`): wait before the
/// first poll, honor `slow_down` (server interval or +5 s), stop at the
/// deadline. Runs as a task on the plugin runtime.
async fn poll_device_tokens(host: &str, device: &Value) -> Result<Value, String> {
    let expires_in = device["expires_in"].as_u64().unwrap_or(600);
    let mut interval = Duration::from_secs(
        device["interval"]
            .as_u64()
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS)
            .max(1),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    let wait = interval.min(deadline.saturating_duration_since(std::time::Instant::now()));
    tokio::time::sleep(wait).await;
    let mut slow_downs = 0u32;
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let (status, body) = post_form(
            host,
            "/oauth2/token",
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", XAI_CLIENT_ID),
                ("device_code", device["device_code"].as_str().unwrap_or("")),
            ],
        )
        .await
        .map_err(|e| format!("xAI OAuth device token polling request failed: {e}"))?;
        match status {
            200..299 => {
                return credentials_from_token_response(&body, None)
                    .map_err(|e| format!("xAI OAuth device token parse failed: {e}"));
            }
            _ => {
                let error = body.get("error").and_then(Value::as_str);
                match error {
                    Some("authorization_pending") => {}
                    Some("slow_down") => {
                        slow_downs += 1;
                        let server_interval = body
                            .get("interval")
                            .and_then(Value::as_u64)
                            .filter(|v| *v > 0);
                        interval = match server_interval {
                            Some(v) => Duration::from_secs(v.max(1)),
                            None => interval + Duration::from_secs(5),
                        };
                    }
                    Some("access_denied") => {
                        return Err("xAI device authorization was denied".to_string());
                    }
                    Some("expired_token") => return Err("xAI device code expired".to_string()),
                    _ => {
                        return Err(request_failure("device token polling", status, &body));
                    }
                }
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(interval.min(remaining)).await;
    }
    if slow_downs > 0 {
        Err("Device flow timed out after one or more slow_down responses. This is often caused by clock drift in WSL or VM environments. Please sync or restart the VM clock and try again.".to_string())
    } else {
        Err("Device flow timed out".to_string())
    }
}

/// One refresh (upstream `refreshXaiToken`): `grant_type=refresh_token`.
/// Synchronous (the core's refresh call); returns the rotated credential.
async fn refresh_tokens(host: &str, refresh_token: &str) -> Result<Value, String> {
    let (status, body) = post_form(
        host,
        "/oauth2/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", XAI_CLIENT_ID),
            ("refresh_token", refresh_token),
        ],
    )
    .await
    .map_err(|e| format!("xAI OAuth token refresh request failed: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(request_failure("token refresh", status, &body));
    }
    credentials_from_token_response(&body, Some(refresh_token))
        .map_err(|e| format!("xAI OAuth token refresh failed: {e}"))
}

/// `auth_start`: a refresh request
/// (`{"action":"refresh","credential":{...}}`) is answered synchronously;
/// an empty request starts the device-code login (the poll task completes
/// the `done` status).
extern "C" fn auth_start(request: *const u8) -> *const u8 {
    let req: Value = serde_json::from_str(&read_cstr(request)).unwrap_or_else(|_| json!({}));
    if req.get("action").and_then(Value::as_str) == Some("refresh") {
        let cred = req.get("credential").cloned().unwrap_or(Value::Null);
        let refresh_token = cred.get("refresh").and_then(Value::as_str);
        let host = oauth_host();
        let res = match refresh_token {
            Some(r) => runtime().block_on(refresh_tokens(&host, r)),
            None => Err("the stored credential has no refresh token".to_string()),
        };
        return match res {
            Ok(credential) => out(&json!({"state": "done", "credential": credential}).to_string()),
            Err(e) => out(&json!({"state": "failed", "error": e}).to_string()),
        };
    }
    // Start a fresh device-code login.
    *AUTH.lock().unwrap() = AuthState {
        url: None,
        code: None,
        credential: None,
        failed: None,
    };
    let host = oauth_host();
    let (device, url) = match runtime().block_on(device_authorization(&host)) {
        Ok(x) => x,
        Err(e) => {
            return out(&json!({"state": "failed", "error": e}).to_string());
        }
    };
    let user_code = device
        .get("user_code")
        .and_then(Value::as_str)
        .map(str::to_string);
    runtime().spawn({
        let host = host.clone();
        async move {
            match poll_device_tokens(&host, &device).await {
                Ok(credential) => {
                    let mut g = AUTH.lock().unwrap();
                    g.credential = Some(credential);
                }
                Err(e) => {
                    let mut g = AUTH.lock().unwrap();
                    g.failed = Some(e);
                }
            }
        }
    });
    {
        let mut g = AUTH.lock().unwrap();
        if let Some(u) = url {
            g.url = Some(u);
        }
        g.code = user_code;
    }
    let (url, code) = {
        let g = AUTH.lock().unwrap();
        (g.url.clone(), g.code.clone())
    };
    out(&json!({
        "state": "pending",
        "url": url,
        "code": code,
    })
    .to_string())
}

/// Polls the handshake: `pending` (with `url` while waiting), `done` (with
/// the `credential` the core stores in auth.json), or `failed`.
extern "C" fn auth_status(_request: *const u8) -> *const u8 {
    let g = AUTH.lock().unwrap();
    if let Some(c) = &g.credential {
        return out(&json!({"state": "done", "credential": c}).to_string());
    }
    if let Some(f) = &g.failed {
        return out(&json!({"state": "failed", "error": f}).to_string());
    }
    let url = g.url.as_ref().cloned();
    let code = g.code.clone();
    out(&json!({ "state": "pending", "url": url, "code": code }).to_string())
}

extern "C" fn dispose() {
    STREAMS.lock().unwrap().clear();
    *AUTH.lock().unwrap() = AuthState {
        url: None,
        code: None,
        credential: None,
        failed: None,
    };
}
