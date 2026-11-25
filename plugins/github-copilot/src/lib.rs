//! GitHub Copilot provider plugin — port of the upstream
//! `githubCopilotProvider` (pinned upstream 64a15f4) for the pi provider
//! plugin ABI v1 (`docs/provider-plugin-abi.md`).
//!
//! **Wire APIs** (dispatched per model, all on the account base URL):
//! `openai-completions` → `POST {base}/chat/completions` (core OpenAI
//! driver shape: messages/tools, `stream_options.include_usage`, SSE
//! `choices[0].delta`), `openai-responses` → `POST {base}/responses`
//! (upstream `api/openai-responses.ts`), `anthropic-messages` →
//! `POST {base}/messages` (upstream `api/anthropic-messages.ts`:
//! `anthropic-version: 2023-06-01`, SSE `content_block_*` events).
//! All three carry the copilot dynamic headers (upstream
//! `api/github-copilot-headers.ts`): `X-Initiator` (user/agent from the
//! last message), `Openai-Intent: conversation-edits`, and
//! `Copilot-Vision-Request: true` when any user/toolResult message has
//! image blocks — plus the account `COPILOT_HEADERS` (User-Agent
//! GitHubCopilotChat/0.35.0, Editor-Version, Editor-Plugin-Version,
//! Copilot-Integration-Id).
//!
//! **Base URL** (upstream `getGitHubCopilotBaseUrl`): the mock hook
//! `GITHUB_COPILOT_BASE_URL` (winning for exchange/models/enable/stream),
//! else the `proxy-ep` parsed from the access token
//! (`tid=...;proxy-ep=proxy.x...` → `https://api.x`), else
//! `https://copilot-api.{enterpriseUrl}` for enterprise logins, else
//! `https://api.individual.githubcopilot.com`.
//!
//! **OAuth** (upstream `auth/oauth/github-copilot.ts`): GitHub device
//! code (`POST {host}/login/device/code` + `POST {host}/login/oauth/
//! access_token`, client id `Iv1.b507a08c87ecfe98`, scope `read:user`) →
//! Copilot token exchange (`POST {base}/copilot_internal/v2/token` with
//! the GitHub token) → model catalog fetch (`GET {base}/models`,
//! `X-GitHub-Api-Version: 2026-06-01`) → best-effort policy enable for
//! `unconfigured` catalog models (`POST {base}/models/{id}/policy`).
//! Hosts: `GITHUB_COPILOT_DEVICE_HOST` (mock hook; else the domain —
//! `github.com`, or the enterprise domain). The credential the core
//! stores (upstream shape): `{type: "oauth", access, refresh:
//! <github token>, expires, availableModelIds, enterpriseUrl?}` —
//! `expires` is the RAW `expires_at*1000` (upstream bakes a 5-minute
//! skew at store time; the core's 5-minute validity margin applies the
//! same rule uniformly — one place, E3S4 semantics).
//!
//! **Refresh** (E4S2 core-side refresh, upstream
//! `refreshGitHubCopilotToken`): the core sends `auth_start` the request
//! `{"action":"refresh","credential":{...}}`; this plugin re-exchanges
//! the stored GitHub token and re-fetches the model catalog, answering
//! `done` (rotated credential) or `failed` synchronously. The core
//! writes the rotation back to auth.json — the plugin never touches the
//! store.
//!
//! **v0 limits** (documented port deviations): the enterprise domain is
//! taken from `GITHUB_COPILOT_ENTERPRISE` (upstream prompts for it — the
//! ABI login has no prompts); the catalog is the pinned static set (the
//! sidecar's account-level `filterModels` against
//! `availableModelIds` has no plugin equivalent — `list_models` takes no
//! credential — so policy-blocked models are selectable but fail with a
//! typed provider error at stream time); the stream request carries no
//! temperature/reasoning/toolChoice/session (the ABI v0 request shape);
//! tool calls ride the conversation (the system message's `toolsAdded`);
//! mid-conversation system messages are dropped (anthropic wire); the
//! `typesafe-system-one` classifier model is not in the catalog (the
//! core has no classifier path); the 429 retry budget on the models
//! fetch is 0 (upstream: 2) — a 429 there is a typed failure.

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

const ABI_VERSION: u32 = 1;

// Upstream constants (pinned 64a15f4, auth/oauth/github-copilot.ts +
// api/github-copilot-headers.ts).
/// `atob("SXYxLmI1MDdhMDhjODdlY2ZlOTg=")` — the GitHub App client id.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEVICE_SCOPE: &str = "read:user";
const COPILOT_API_VERSION: &str = "2026-06-01";
const DEFAULT_BASE: &str = "https://api.individual.githubcopilot.com";
const DEFAULT_DOMAIN: &str = "github.com";
/// The account headers every copilot request carries (upstream
/// `COPILOT_HEADERS`; identical across all catalog model entries).
const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
/// The RFC 8628 fallback poll interval (upstream shared poller).
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry-shaped catalog (pinned upstream data/github-copilot.json:
/// all chat models across the three wire APIs; the `typesafe-system-one`
/// classifier is excluded — the core has no classifier path).
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

static NAME_BYTES: &[u8] = b"github-copilot\0";

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
            .expect("github-copilot plugin: tokio runtime")
    })
}

/// The account base URL for a stream (upstream `getGitHubCopilotBaseUrl`
/// + the mock hook; the credential carries `access`/`enterpriseUrl`).
fn stream_base(credential: &Value) -> String {
    if let Ok(b) = std::env::var("GITHUB_COPILOT_BASE_URL") {
        if !b.trim().is_empty() {
            return b.trim().trim_end_matches('/').to_string();
        }
    }
    if let Some(access) = credential.get("access").and_then(Value::as_str) {
        if let Some(b) = base_url_from_token(access) {
            return b;
        }
    }
    if let Some(dom) = credential
        .get("enterpriseUrl")
        .and_then(Value::as_str)
        .and_then(normalize_domain)
    {
        return format!("https://copilot-api.{dom}");
    }
    DEFAULT_BASE.to_string()
}

/// Upstream `getBaseUrlFromToken`: `proxy-ep=proxy.x.y` → `https://api.x.y`.
fn base_url_from_token(token: &str) -> Option<String> {
    let rest = token.split("proxy-ep=").nth(1)?;
    let proxy_host = rest.split(';').next()?.trim();
    if proxy_host.is_empty() {
        return None;
    }
    let api_host = proxy_host.strip_prefix("proxy.").unwrap_or(proxy_host);
    Some(format!("https://{api_host}"))
}

/// Upstream `normalizeDomain` subset: a domain or URL → hostname.
fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let host_part = if let Some(idx) = trimmed.find("://") {
        trimmed[idx + 3..].to_string()
    } else {
        trimmed.to_string()
    };
    let host = host_part
        .split('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    // A port or userinfo would be an invalid enterprise domain for
    // upstream's URL-based flow too (the hostname is what it keeps).
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    if host.is_empty() || host.contains("://") {
        return None;
    }
    Some(host.split(':').next().unwrap_or(host).to_string())
}

/// The copilot dynamic headers (upstream `buildCopilotDynamicHeaders`).
fn copilot_dynamic_headers(convo: &[Value]) -> Vec<(&'static str, String)> {
    let last_role = convo
        .last()
        .and_then(|m| m.get("role").and_then(Value::as_str))
        .unwrap_or("user");
    let initiator = if last_role != "user" { "agent" } else { "user" };
    let has_images = convo.iter().any(|m| {
        let role = m.get("role").and_then(Value::as_str);
        matches!(role, Some("user") | Some("toolResult"))
            && m.get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                })
                .unwrap_or(false)
    });
    let mut h: Vec<(&'static str, String)> = vec![
        ("X-Initiator", initiator.to_string()),
        ("Openai-Intent", "conversation-edits".to_string()),
    ];
    if has_images {
        h.push(("Copilot-Vision-Request", "true".to_string()));
    }
    h
}

fn client_with_copilot_headers() -> Result<reqwest::Client, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    Ok(client)
}

/// `{"handle": ...}` on success, `{"error": ...}` on a request-level
/// failure (wire failures surface as stream events, not open failures).
extern "C" fn stream_open(request: *const u8) -> *const u8 {
    let req: Value = match serde_json::from_str(&read_cstr(request)) {
        Ok(v) => v,
        Err(e) => return err(&format!("stream request is not valid JSON: {e}")),
    };
    let model = req.get("model").cloned().unwrap_or(Value::Null);
    let model_id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let api = model
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // The core serializes the registry model camelCase (`rename_all`).
    let max_tokens = model.get("maxTokens").and_then(Value::as_u64);
    let reasoning = model
        .get("reasoning")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let messages = req.get("messages").cloned().unwrap_or(Value::Array(vec![]));
    let credential = req.get("credential").cloned().unwrap_or(Value::Null);
    // The core resolved the credential (auth.json / login / refresh); the
    // plugin only consumes it here — it is never stored plugin-side.
    let token = credential
        .get("key")
        .or_else(|| credential.get("access"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let base = stream_base(&credential);
    if token.is_none() {
        return err("no credential token for the stream request");
    }
    let token = token.unwrap();
    if !matches!(
        api.as_str(),
        "openai-completions" | "openai-responses" | "anthropic-messages"
    ) {
        return err(&format!(
            "model {model_id} has wire api {api}, which the github-copilot plugin does not serve"
        ));
    }

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
        let convo: Vec<Value> = messages.as_array().cloned().unwrap_or_default();
        let inner = runtime().spawn(async move {
            match api.as_str() {
                "openai-completions" => {
                    stream_completions(&base, &model_id, &convo, &token, tx).await
                }
                "openai-responses" => {
                    stream_responses(&base, &model_id, reasoning, &convo, &token, tx).await
                }
                "anthropic-messages" => {
                    stream_anthropic(&base, &model_id, max_tokens, &convo, &token, tx).await
                }
                _ => unreachable!("validated above"),
            }
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
// Shared message helpers (pi conversation → wire shapes)

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

/// The system message's `toolsAdded` (the core's tools channel).
fn tools_added(convo: &[Value]) -> Vec<Value> {
    convo
        .iter()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("system"))
        .and_then(|m| m.get("toolsAdded").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

/// A `done` turn from text + tool calls + usage.
fn send_done(
    tx: &tokio::sync::mpsc::UnboundedSender<Value>,
    text: String,
    tool_calls: Vec<Value>,
    usage: Value,
) {
    let _ = tx.send(json!({
        "type": "done",
        "turn": {
            "text": if text.is_empty() { Value::Null } else { Value::String(text) },
            "tool_calls": tool_calls,
            "usage": usage,
        },
    }));
}

fn send_error(tx: &tokio::sync::mpsc::UnboundedSender<Value>, msg: String) {
    let _ = tx.send(json!({"type": "error", "error": msg}));
}

// ---------------------------------------------------------------------------
// Wire 1: openai-completions (the core OpenAiDriver shape — messages,
// tools, `stream_options.include_usage`, SSE `choices[0].delta`)

/// Upstream/core `mapMessages`: the pi conversation → chat/completions
/// messages (system string, user text/image data-URL parts, assistant
/// text + `tool_calls`, toolResult → `tool`).
fn completions_messages(convo: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in convo {
        match m.get("role").and_then(Value::as_str) {
            Some("system") => {
                if let Some(c) = m.get("content").and_then(Value::as_str) {
                    out.push(json!({ "role": "system", "content": c }));
                }
            }
            Some("user") => {
                let images: Vec<(String, String)> = m
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                            .filter_map(|b| {
                                Some((
                                    b.get("mimeType")
                                        .and_then(Value::as_str)
                                        .unwrap_or("image/png")
                                        .to_string(),
                                    b.get("data").and_then(Value::as_str)?.to_string(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if images.is_empty() {
                    out.push(json!({ "role": "user", "content": text_of(m.get("content")) }));
                } else {
                    let mut parts: Vec<Value> = Vec::new();
                    let text = text_of(m.get("content"));
                    if !text.is_empty() {
                        parts.push(json!({ "type": "text", "text": text }));
                    }
                    for (mime, data) in &images {
                        parts.push(json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{mime};base64,{data}") },
                        }));
                    }
                    out.push(json!({ "role": "user", "content": parts }));
                }
            }
            Some("assistant") => {
                let mut text = String::new();
                let mut tool_calls = Vec::new();
                if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let t = b.get("text").and_then(Value::as_str).unwrap_or_default();
                                if !text.is_empty() {
                                    text.push(' ');
                                }
                                text.push_str(t);
                            }
                            Some("toolCall") => tool_calls.push(json!({
                                "id": b.get("id").cloned().unwrap_or(Value::Null),
                                "type": "function",
                                "function": {
                                    "name": b.get("name").cloned().unwrap_or(Value::Null),
                                    "arguments": b.get("input").or_else(|| b.get("arguments")).map(|v| v.to_string()).unwrap_or_default(),
                                },
                            })),
                            _ => {}
                        }
                    }
                }
                let mut msg = json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { Value::String(text) },
                });
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                out.push(msg);
            }
            Some("toolResult") => out.push(json!({
                "role": "tool",
                "tool_call_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
                "content": text_of(m.get("content")),
            })),
            _ => {}
        }
    }
    out
}

async fn stream_completions(
    base: &str,
    model_id: &str,
    convo: &[Value],
    token: &str,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let fail = |msg: String| send_error(&tx, msg);
    let client = match client_with_copilot_headers() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let tools: Vec<Value> = tools_added(convo)
        .iter()
        .filter_map(|d| {
            let name = d.get("name").and_then(Value::as_str)?;
            Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": d.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": d.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
                },
            }))
        })
        .collect();
    let mut body = json!({
        "model": model_id,
        "stream": true,
        "stream_options": { "include_usage": true },
        "messages": completions_messages(convo),
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let mut reqb = client
        .post(format!("{base}/chat/completions"))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .json(&body);
    for (k, v) in COPILOT_HEADERS {
        reqb = reqb.header(k, v);
    }
    for (k, v) in copilot_dynamic_headers(convo) {
        reqb = reqb.header(k, v);
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
    let mut text = String::new();
    let mut usage = json!({"input": 0, "output": 0, "cache_read": 0, "cache_write": 0});
    // index -> (id, name, accumulated argument JSON string)
    let mut calls: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return fail(format!("stream error: {e}")),
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].to_string();
            buf.drain(..=pos);
            let line = line.trim_end_matches('\r').trim().to_string();
            let Some(payload) = line.strip_prefix("data:").map(|p| p.trim().to_string()) else {
                continue;
            };
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            if let Some(e) = v.get("error") {
                return fail(format!("openai stream error: {e}"));
            }
            if let Some(u) = v.get("usage") {
                usage["input"] = json!(u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0));
                usage["output"] = json!(u
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0));
                usage["cache_read"] = json!(u
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0));
                usage["cache_write"] = json!(u
                    .pointer("/prompt_tokens_details/cache_creation_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0));
            }
            let Some(delta) = v.pointer("/choices/0/delta") else {
                continue;
            };
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                text.push_str(t);
                let _ = tx.send(json!({"type": "delta", "delta": t}));
            }
            if let Some(items) = delta.get("tool_calls").and_then(Value::as_array) {
                for c in items {
                    let idx = c.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let e = calls.entry(idx).or_default();
                    if let Some(id) = c.get("id").and_then(Value::as_str) {
                        e.0 = id.to_string();
                    }
                    if let Some(n) = c.pointer("/function/name").and_then(Value::as_str) {
                        e.1 = n.to_string();
                    }
                    if let Some(f) = c
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                    {
                        e.2.push_str(&f);
                    }
                }
            }
        }
    }
    let tool_calls: Vec<Value> = calls
        .into_iter()
        .map(|(_, (id, name, args))| {
            let input: Value = if args.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&args).unwrap_or(json!({}))
            };
            json!({ "id": id, "name": name, "input": input })
        })
        .collect();
    send_done(&tx, text, tool_calls, usage);
}

// ---------------------------------------------------------------------------
// Wire 2: openai-responses (upstream `api/openai-responses.ts` +
// `openai-responses-shared.ts`)

/// Upstream `convertResponsesMessages` subset: the pi conversation →
/// Responses `input` items. v0: system → one `developer` item (upstream
/// role for reasoning models; `system` otherwise), user text/image,
/// assistant text + `function_call`, `toolResult` → `function_call_output`.
fn responses_messages(model_reasoning: bool, convo: &[Value]) -> Vec<Value> {
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
            Some("toolResult") => out.push(json!({
                "type": "function_call_output",
                "call_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
                "output": text_of(m.get("content")),
            })),
            _ => {}
        }
    }
    out
}

/// The system message's `toolsAdded` → Responses `tools` (upstream
/// `convertResponsesTools` subset: name/description/parameters).
fn responses_tools(convo: &[Value]) -> Vec<Value> {
    tools_added(convo)
        .iter()
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

async fn stream_responses(
    base: &str,
    model_id: &str,
    reasoning: bool,
    convo: &[Value],
    token: &str,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let fail = |msg: String| send_error(&tx, msg);
    let client = match client_with_copilot_headers() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let tools = responses_tools(convo);
    let mut body = json!({
        "model": model_id,
        "input": responses_messages(reasoning, convo),
        "stream": true,
        "store": false,
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let mut reqb = client
        .post(format!("{base}/responses"))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .json(&body);
    for (k, v) in COPILOT_HEADERS {
        reqb = reqb.header(k, v);
    }
    for (k, v) in copilot_dynamic_headers(convo) {
        reqb = reqb.header(k, v);
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
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Slot {
        Text,
        ToolCall,
    }
    let mut slots: HashMap<u64, Slot> = HashMap::new();
    let mut text_full = String::new();
    // tool call: output_index -> (id, name, raw-arguments accumulator)
    let mut tools_acc: HashMap<u64, (String, String, String, bool)> = HashMap::new();
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
                            tools_acc.insert(
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
                // Reasoning items are not ported (v0: no thinking support
                // in the ABI); their deltas carry nothing to the turn.
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
                    if let Some(t) = tools_acc.get_mut(&idx) {
                        t.2.push_str(ev.get("delta").and_then(Value::as_str).unwrap_or_default());
                    }
                }
                "response.function_call_arguments.done" => {
                    let idx = ev.get("output_index").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(t) = tools_acc.get_mut(&idx) {
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
                        if let Some(t) = tools_acc.get_mut(&idx) {
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
    let tool_calls: Vec<Value> = tools_acc
        .values()
        .filter(|(_, _, _, done)| *done)
        .map(|(id, name, args, _)| {
            let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
            json!({ "id": id, "name": name, "input": input })
        })
        .collect();
    send_done(&tx, text_full, tool_calls, usage);
}

// ---------------------------------------------------------------------------
// Wire 3: anthropic-messages (upstream `api/anthropic-messages.ts`)

/// The pi conversation → Anthropic Messages request pieces: `system`
/// (the first system message; mid-conversation system messages are
/// dropped in v0), `messages`, and the tools (from `toolsAdded`).
fn anthropic_request(convo: &[Value]) -> (Option<String>, Vec<Value>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut out = Vec::new();
    for m in convo {
        match m.get("role").and_then(Value::as_str) {
            Some("system") => {
                if system.is_none() {
                    if let Some(c) = m.get("content").and_then(Value::as_str) {
                        system = Some(c.to_string());
                    }
                }
            }
            Some("user") => match m.get("content") {
                Some(Value::String(s)) => {
                    out.push(json!({ "role": "user", "content": s }));
                }
                Some(Value::Array(blocks)) => {
                    let content: Vec<Value> = blocks
                        .iter()
                        .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                            Some("text") => b
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|t| json!({ "type": "text", "text": t })),
                            Some("image") => {
                                let media_type = b
                                    .get("mimeType")
                                    .and_then(Value::as_str)
                                    .unwrap_or("image/png");
                                let data = b.get("data")?.as_str()?;
                                Some(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    },
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
                let mut content: Vec<Value> = Vec::new();
                if let Some(blocks) = m.get("content").and_then(Value::as_array) {
                    for b in blocks {
                        match b.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(Value::as_str) {
                                    content.push(json!({ "type": "text", "text": t }));
                                }
                            }
                            Some("toolCall") => {
                                let input: Value = b
                                    .get("input")
                                    .or_else(|| b.get("arguments"))
                                    .cloned()
                                    .unwrap_or(json!({}));
                                content.push(json!({
                                    "type": "tool_use",
                                    "id": b.get("id").cloned().unwrap_or(Value::Null),
                                    "name": b.get("name").cloned().unwrap_or(Value::Null),
                                    "input": input,
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                if !content.is_empty() {
                    out.push(json!({ "role": "assistant", "content": content }));
                }
            }
            Some("toolResult") => {
                out.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": m.get("toolCallId").cloned().unwrap_or(Value::Null),
                        "content": text_of(m.get("content")),
                    }],
                }));
            }
            _ => {}
        }
    }
    let tools: Vec<Value> = tools_added(convo)
        .iter()
        .filter_map(|d| {
            let name = d.get("name").and_then(Value::as_str)?;
            Some(json!({
                "name": name,
                "description": d.get("description").cloned().unwrap_or(Value::Null),
                "input_schema": d.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
            }))
        })
        .collect();
    (system, out, tools)
}

async fn stream_anthropic(
    base: &str,
    model_id: &str,
    max_tokens: Option<u64>,
    convo: &[Value],
    token: &str,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let fail = |msg: String| send_error(&tx, msg);
    let max_tokens = match max_tokens {
        Some(n) if n > 0 => n,
        _ => {
            return fail(format!(
                "model {model_id} has no max_tokens in the registry — the Anthropic API requires max_tokens"
            ))
        }
    };
    let (system, messages, tools) = anthropic_request(convo);
    let client = match client_with_copilot_headers() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut body = json!({
        "model": model_id,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });
    if let Some(s) = &system {
        body["system"] = json!(s);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    let mut reqb = client
        .post(format!("{base}/messages"))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-dangerous-direct-browser-access", "true")
        .json(&body);
    for (k, v) in COPILOT_HEADERS {
        reqb = reqb.header(k, v);
    }
    for (k, v) in copilot_dynamic_headers(convo) {
        reqb = reqb.header(k, v);
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
    #[derive(Debug)]
    enum Block {
        Text(String),
        Tool {
            id: String,
            name: String,
            args: String,
            done: bool,
        },
    }
    let mut blocks: std::collections::BTreeMap<u64, Block> = std::collections::BTreeMap::new();
    let mut usage = json!({"input": 0, "output": 0, "cache_read": 0, "cache_write": 0});
    let mut saw_message_delta = false;

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
            let Some(payload) = line.strip_prefix("data:").map(|p| p.trim().to_string()) else {
                continue;
            };
            if payload.is_empty() {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            let etype = ev.get("type").and_then(Value::as_str).unwrap_or_default();
            match etype {
                "message_start" => {
                    if let Some(u) = ev.pointer("/message/usage") {
                        usage["input"] =
                            json!(u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0));
                        usage["cache_read"] = json!(u
                            .get("cache_read_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0));
                        usage["cache_write"] = json!(u
                            .get("cache_creation_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0));
                    }
                }
                "content_block_start" => {
                    let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let btype = ev
                        .pointer("/content_block/type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    match btype {
                        "text" => {
                            blocks.insert(idx, Block::Text(String::new()));
                        }
                        "tool_use" => {
                            blocks.insert(
                                idx,
                                Block::Tool {
                                    id: ev
                                        .pointer("/content_block/id")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    name: ev
                                        .pointer("/content_block/name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    args: String::new(),
                                    done: false,
                                },
                            );
                        }
                        "thinking" => {
                            // Not ported (v0: no thinking support in the ABI).
                        }
                        _ => {}
                    }
                }
                "content_block_delta" => {
                    let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0);
                    let dtype = ev
                        .pointer("/delta/type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    match dtype {
                        "text_delta" => {
                            let d = ev
                                .pointer("/delta/text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if let Some(Block::Text(t)) = blocks.get_mut(&idx) {
                                t.push_str(d);
                                let _ = tx.send(json!({"type": "delta", "delta": d}));
                            }
                        }
                        "input_json_delta" => {
                            let d = ev
                                .pointer("/delta/partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if let Some(Block::Tool {
                                args, done: false, ..
                            }) = blocks.get_mut(&idx)
                            {
                                args.push_str(d);
                            }
                        }
                        "thinking_delta" => {}
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let idx = ev.get("index").and_then(Value::as_u64).unwrap_or(0);
                    // `input_json_delta` is the only tool payload in the
                    // streaming API; the block is complete now.
                    if let Some(Block::Tool { done, .. }) = blocks.get_mut(&idx) {
                        *done = true;
                    }
                }
                "message_delta" => {
                    saw_message_delta = true;
                    if let Some(u) = ev.get("usage") {
                        usage["output"] =
                            json!(u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0));
                    }
                }
                "message_stop" => {
                    if saw_message_delta {
                        break;
                    }
                }
                "error" => {
                    let msg = ev
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("anthropic stream error");
                    return fail(msg.to_string());
                }
                _ => {} // ping / unknown events: ignore (upstream)
            }
        }
        if saw_message_delta {
            break;
        }
    }
    if !saw_message_delta {
        return fail("anthropic stream ended before message_delta".to_string());
    }
    let text: String = blocks
        .values()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.as_str()),
            Block::Tool { .. } => None,
        })
        .collect();
    let tool_calls: Vec<Value> = blocks
        .values()
        .filter_map(|b| match b {
            Block::Tool {
                id,
                name,
                args,
                done,
            } => {
                // `content_block_stop` marks completeness (see above).
                if *done {
                    let input: Value = if args.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(args).unwrap_or(json!({}))
                    };
                    Some(json!({ "id": id, "name": name, "input": input }))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    send_done(&tx, text, tool_calls, usage);
}

// ---------------------------------------------------------------------------
// OAuth: device flow + exchange + models + enable (upstream
// `auth/oauth/github-copilot.ts`)

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

/// Upstream `trustedHttpUrl`: trusted http(s) (github.com's device flow
/// verification URI is https; self-hosted GHE may be http).
fn trusted_http_url(raw: &str) -> Result<String, String> {
    let ok = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .filter(|rest| !rest.is_empty())
        .is_some();
    if ok {
        Ok(raw.to_string())
    } else {
        Err("Untrusted verification_uri in device code response".to_string())
    }
}

/// The device-flow host (upstream: `https://{domain}`; the mock hook
/// replaces it wholesale, including the scheme).
fn device_host(enterprise_domain: &str) -> String {
    if let Ok(h) = std::env::var("GITHUB_COPILOT_DEVICE_HOST") {
        if !h.trim().is_empty() {
            let h = h.trim();
            return if h.contains("://") {
                h.to_string()
            } else {
                format!("https://{h}")
            }
            .trim_end_matches('/')
            .to_string();
        }
    }
    format!("https://{enterprise_domain}")
}

/// The Copilot token exchange URL (upstream `getUrls`:
/// `https://api.{domain}/copilot_internal/v2/token`; the mock hook wins).
fn exchange_url(enterprise_domain: &str) -> String {
    if let Ok(b) = std::env::var("GITHUB_COPILOT_BASE_URL") {
        if !b.trim().is_empty() {
            return format!(
                "{}/copilot_internal/v2/token",
                b.trim().trim_end_matches('/')
            );
        }
    }
    format!("https://api.{enterprise_domain}/copilot_internal/v2/token")
}

async fn post_form_json(
    url: &str,
    fields: &[(&str, &str)],
    ua: bool,
) -> Result<(u16, Value), String> {
    let client = client_with_copilot_headers()?;
    let mut reqb = client.post(url).form(fields);
    if ua {
        reqb = reqb.header("User-Agent", COPILOT_HEADERS[0].1);
    }
    let resp = reqb
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Copilot OAuth request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let body = if body.is_object() { body } else { json!({}) };
    Ok((status, body))
}

async fn start_device_flow(host: &str) -> Result<(Value, Option<String>), String> {
    let (status, body) = post_form_json(
        &format!("{host}/login/device/code"),
        &[("client_id", CLIENT_ID), ("scope", DEVICE_SCOPE)],
        true,
    )
    .await
    .map_err(|e| format!("Copilot device code request failed: {e}"))?;
    if !(200..300).contains(&status) {
        let detail: Vec<String> = [
            body.get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
            body.get("error_description")
                .and_then(Value::as_str)
                .map(|d| format!(": {d}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        return Err(if detail.is_empty() {
            format!("Copilot device code request failed (HTTP {status})")
        } else {
            format!(
                "Copilot device code request failed (HTTP {}): {}",
                status,
                detail.join(" ")
            )
        });
    }
    let device_code = body
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid device code response fields".to_string())?;
    // The user code is surfaced in the pending status (`code` field — the
    // ABI v1 pending payload extension, E4S2); the core prints it.
    let user_code = body
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid device code response fields".to_string())?;
    let verification_uri = trusted_http_url(
        body.get("verification_uri")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Invalid device code response fields".to_string())?,
    )?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_i64)
        .ok_or_else(|| "Invalid device code response fields".to_string())?;
    let interval = body
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0);
    Ok((
        json!({
            "device_code": device_code,
            "user_code": user_code,
            "expires_in": expires_in,
            "interval": interval,
        }),
        Some(verification_uri),
    ))
}

/// The device-code poll loop (upstream `pollForGitHubAccessToken` via the
/// shared `pollOAuthDeviceCodeFlow`, `waitBeforeFirstPoll: true`): wait
/// before the first poll, honor `slow_down` (server interval or +5 s),
/// stop at the deadline.
async fn poll_github_access_token(host: &str, device: &Value) -> Result<String, String> {
    let deadline = std::time::Instant::now()
        + Duration::from_secs(device["expires_in"].as_i64().unwrap_or(600).max(0) as u64);
    let mut interval = Duration::from_secs(
        device
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS)
            .max(1),
    );
    let wait = interval.min(deadline.saturating_duration_since(std::time::Instant::now()));
    tokio::time::sleep(wait).await;
    let mut slow_downs = 0u32;
    loop {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let (status, body) = post_form_json(
            &format!("{host}/login/oauth/access_token"),
            &[
                ("client_id", CLIENT_ID),
                ("device_code", device["device_code"].as_str().unwrap_or("")),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
            true,
        )
        .await
        .map_err(|e| format!("Copilot access token polling request failed: {e}"))?;
        if (200..299).contains(&status) {
            if let Some(token) = body
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                return Ok(token.to_string());
            }
        }
        match body.get("error").and_then(Value::as_str) {
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
            Some(e) => {
                let suffix = body
                    .get("error_description")
                    .and_then(Value::as_str)
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default();
                return Err(format!("Device flow failed: {e}{suffix}"));
            }
            None => {
                if !(200..299).contains(&status) {
                    return Err(format!(
                        "Copilot access token polling failed (HTTP {status})"
                    ));
                }
                return Err("Invalid device token response".to_string());
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

/// Upstream `refreshGitHubCopilotAccessToken`: exchange a GitHub token
/// for a Copilot token. Returns the credential (with `enterpriseUrl`
/// carried through when the login was enterprise).
async fn exchange(github_token: &str, enterprise_domain: &str) -> Result<Value, String> {
    let client = client_with_copilot_headers()?;
    let resp = client
        .post(exchange_url(enterprise_domain))
        .bearer_auth(github_token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Copilot token exchange request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let body = if body.is_object() { body } else { json!({}) };
    if !(200..300).contains(&status) {
        let text = body.to_string();
        return Err(format!(
            "Copilot token exchange failed with status {status}: {}",
            text.chars().take(160).collect::<String>()
        ));
    }
    let token = body
        .get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid Copilot token response fields".to_string())?;
    let expires_at = body
        .get("expires_at")
        .and_then(Value::as_i64)
        .ok_or_else(|| "Invalid Copilot token response fields".to_string())?;
    let mut cred = json!({
        "type": "oauth",
        "refresh": github_token,
        "access": token,
        "expires": expires_at * 1000,
    });
    if enterprise_domain != DEFAULT_DOMAIN {
        cred["enterpriseUrl"] = json!(enterprise_domain);
    }
    Ok(cred)
}

/// The static catalog ids (for the policy-fallback bookkeeping, upstream
/// `GITHUB_COPILOT_MODELS`).
fn catalog_ids() -> Vec<String> {
    serde_json::from_str::<Vec<Value>>(CATALOG)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Upstream `fetchGitHubCopilotModels` + `parseGitHubCopilotModelCatalog`
/// (`allowPolicyFallback` limited to the individual default base, upstream
/// rule; the 429 retry budget is 0 in v0 — a typed failure).
async fn fetch_models(
    access_token: &str,
    enterprise_domain: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    // The base follows the token (proxy-ep) → enterprise → default, same
    // as upstream `getGitHubCopilotBaseUrl`; the mock hook wins.
    let base = {
        let cred = json!({ "access": access_token, "enterpriseUrl": enterprise_domain });
        stream_base(&cred)
    };
    let allow_policy_fallback = base == DEFAULT_BASE;
    let client = client_with_copilot_headers()?;
    let resp = client
        .get(format!("{base}/models"))
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("X-GitHub-Api-Version", COPILOT_API_VERSION)
        .send()
        .await
        .map_err(|e| format!("Copilot models fetch request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let text = body.to_string();
        return Err(format!(
            "Copilot models fetch failed with status {status}: {}",
            text.chars().take(160).collect::<String>()
        ));
    }
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Invalid Copilot models response".to_string())?;
    let static_ids: Vec<String> = catalog_ids();
    let mut account: Vec<(String, bool, Option<String>)> = Vec::new();
    for raw in data {
        if !raw.is_object() {
            continue;
        }
        let id = match raw.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let supports = raw
            .pointer("/capabilities/supports")
            .and_then(Value::as_object);
        if supports
            .and_then(|s| s.get("tool_calls"))
            .and_then(Value::as_bool)
            == Some(false)
        {
            continue;
        }
        let picker = raw
            .get("model_picker_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let policy = raw
            .pointer("/policy/state")
            .and_then(Value::as_str)
            .map(str::to_string);
        account.push((id, picker, policy));
    }
    let picker_ids: Vec<String> = account
        .iter()
        .filter(|(_, picker, policy)| *picker && !matches!(policy, Some(ref p) if p == "disabled"))
        .map(|(id, _, _)| id.clone())
        .collect();
    let use_policy_fallback = allow_policy_fallback && picker_ids.is_empty();
    let available = if !picker_ids.is_empty() || !allow_policy_fallback {
        picker_ids
    } else {
        account
            .iter()
            .filter(|(_, _, policy)| policy.as_deref() == Some("enabled"))
            .map(|(id, _, _)| id.clone())
            .collect()
    };
    let policy_ids: Vec<String> = account
        .iter()
        .filter(|(id, picker, policy)| {
            policy.as_deref() == Some("unconfigured")
                && static_ids.contains(id)
                && (*picker || use_policy_fallback)
        })
        .map(|(id, _, _)| id.clone())
        .collect();
    Ok((available, policy_ids))
}

/// Upstream `enableGitHubCopilotModel`: best-effort `POST
/// {base}/models/{id}/policy` with a 429 retry budget of 2 (backoff
/// 500*2^retry, `retry-after` seconds honored).
async fn enable_model(access_token: &str, model_id: &str, enterprise_domain: &str) -> bool {
    let base = {
        let cred = json!({ "access": access_token, "enterpriseUrl": enterprise_domain });
        stream_base(&cred)
    };
    let client = match client_with_copilot_headers() {
        Ok(c) => c,
        Err(_) => return false,
    };
    for retry in 0..=2u32 {
        let mut reqb = client
            .post(format!("{base}/models/{model_id}/policy"))
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("openai-intent", "chat-policy")
            .header("x-interaction-type", "chat-policy")
            .body(json!({ "state": "enabled" }).to_string());
        for (k, v) in COPILOT_HEADERS {
            reqb = reqb.header(k, v);
        }
        let resp = match reqb.send().await {
            Ok(r) => r,
            Err(_) => return false,
        };
        if resp.status().as_u16() != 429 || retry == 2 {
            return resp.status().is_success();
        }
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let delay = match retry_after {
            Some(secs) => Duration::from_secs(secs),
            None => Duration::from_millis(500 * 2u64.pow(retry)),
        };
        tokio::time::sleep(delay).await;
    }
    false
}

/// The login continuation (upstream `loginGitHubCopilot` minus the
/// enterprise prompt — the ABI login has no prompts, `GITHUB_COPILOT_
/// ENTERPRISE` instead, v0 deviation): poll → exchange → models → enable
/// (best effort) → the credential the core stores.
async fn login_continuation(
    device: Value,
    host: String,
    enterprise_domain: String,
) -> Result<Value, String> {
    let github_token = poll_github_access_token(&host, &device).await?;
    let mut cred = exchange(&github_token, &enterprise_domain).await?;
    let access = cred["access"].as_str().unwrap().to_string();
    let (available, policy_ids) = fetch_models(&access, &enterprise_domain).await?;
    let mut enabled = Vec::new();
    for model_id in &policy_ids {
        // Upstream: the batch stops at the first hard failure (rate
        // limiting exhausted) — best effort per model.
        if enable_model(&access, model_id, &enterprise_domain).await {
            enabled.push(model_id.clone());
        } else {
            break;
        }
    }
    let mut available: Vec<String> = available;
    for id in enabled {
        if !available.contains(&id) {
            available.push(id);
        }
    }
    cred["availableModelIds"] = json!(available);
    Ok(cred)
}

/// One refresh (upstream `refreshGitHubCopilotToken`): re-exchange the
/// stored GitHub token + re-fetch the model catalog. Synchronous (the
/// core's refresh call); returns the rotated credential.
async fn refresh_credential(credential: &Value) -> Result<Value, String> {
    let github_token = credential
        .get("refresh")
        .and_then(Value::as_str)
        .ok_or_else(|| "the stored credential has no GitHub refresh token".to_string())?;
    let enterprise_domain = credential
        .get("enterpriseUrl")
        .and_then(Value::as_str)
        .and_then(normalize_domain)
        .unwrap_or_else(|| DEFAULT_DOMAIN.to_string());
    let mut cred = exchange(github_token, &enterprise_domain).await?;
    let access = cred["access"].as_str().unwrap().to_string();
    let (available, _policy) = fetch_models(&access, &enterprise_domain).await?;
    cred["availableModelIds"] = json!(available);
    Ok(cred)
}

/// `auth_start`: a refresh request
/// (`{"action":"refresh","credential":{...}}`) is answered synchronously;
/// an empty request starts the device-code login (the continuation task
/// completes the `done` status).
extern "C" fn auth_start(request: *const u8) -> *const u8 {
    let req: Value = serde_json::from_str(&read_cstr(request)).unwrap_or_else(|_| json!({}));
    if req.get("action").and_then(Value::as_str) == Some("refresh") {
        let cred = req.get("credential").cloned().unwrap_or(Value::Null);
        let res = runtime().block_on(refresh_credential(&cred));
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
    let enterprise_domain = std::env::var("GITHUB_COPILOT_ENTERPRISE")
        .ok()
        .and_then(|v| normalize_domain(&v))
        .unwrap_or_else(|| DEFAULT_DOMAIN.to_string());
    let host = device_host(&enterprise_domain);
    let (device, url) = match runtime().block_on(start_device_flow(&host)) {
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
        let enterprise_domain = enterprise_domain.clone();
        async move {
            match login_continuation(device, host, enterprise_domain).await {
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
