//! Radius provider plugin — port of the upstream `radiusProvider`
//! (pinned upstream 64a15f4) for the pi provider plugin ABI v1
//! (`docs/provider-plugin-abi.md`).
//!
//! **Wire API** (`pi-messages`, upstream `api/pi-messages.ts`): a single
//! `POST {gateway}/v1/messages` of `{model, context: {messages}, options}`
//! — the pi conversation passes through unconverted (that is the point of
//! pi-messages). Events are `\n\n`-framed `data:` JSON: `start`,
//! `text_start/delta/end`, `toolcall_start/delta/end`, `thinking_*`
//! (ignored in v0), `done` (usage in camelCase), `error`.
//! Gateway: `RADIUS_GATEWAY` (mock hook) else `https://radius.pi.dev`.
//!
//! **OAuth** (device code, upstream `auth/oauth/radius.ts`): the client
//! APIs live on the configured gateway — `POST {gw}/v1/oauth/device` +
//! `POST {gw}/v1/oauth/token` (client id `pi-gateway`, scope
//! `gateway offline_access`). The verification URI is validated as
//! trusted http(s) (self-hosted dev gateways may be plain http).
//! The browser/PKCE flow is NOT ported (task scope: device code only —
//! a documented deviation). The credential the core stores (upstream
//! shape, incl. `scope` when present): `{type: "oauth", access, refresh,
//! expires}` — `expires` is the RAW `now + expires_in*1000` (upstream
//! bakes a 60-second skew at store time; the core's 5-minute validity
//! margin applies the same rule uniformly — one place, E3S4 semantics).
//!
//! **Refresh** (E4S2 core-side refresh): the core sends `auth_start` the
//! request `{"action":"refresh","credential":{...}}`; this plugin
//! answers `done` (rotated credential) or `failed` synchronously
//! (`grant_type=refresh_token`). The core writes the rotation back to
//! auth.json — the plugin never touches the store.
//!
//! **v0 limits** (documented port deviations): the stream request's
//! `options` is empty (the ABI v0 request shape carries no
//! temperature/maxTokens/reasoning/cacheRetention/sessionId/toolChoice);
//! thinking blocks are dropped (no thinking support in the ABI); the
//! model catalog is the pinned baseline (the gateway's dynamic catalog
//! replace is not ported — the gateway serves whatever model ids the
//! baseline lists).

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

const ABI_VERSION: u32 = 1;

// Upstream constants (pinned 64a15f4, auth/oauth/radius.ts).
const OAUTH_CLIENT_ID: &str = "pi-gateway";
const OAUTH_SCOPE: &str = "gateway offline_access";
const DEFAULT_GATEWAY: &str = "https://radius.pi.dev";
/// The RFC 8628 fallback poll interval (radius's response may omit it, or
/// report 0 — upstream rule).
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The registry-shaped catalog (pinned upstream data/radius.json, the
/// `pi-messages` chat baseline — the ids the sidecar/upstream advertise).
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

static NAME_BYTES: &[u8] = b"radius\0";

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
            .expect("radius plugin: tokio runtime")
    })
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
    let messages = req.get("messages").cloned().unwrap_or(Value::Array(vec![]));
    let gateway = radius_gateway();
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
            stream_pi_messages(&gateway, &model_id, &messages, token.as_deref(), tx).await;
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
// The pi-messages wire (upstream `api/pi-messages.ts`: request payload,
// `readPiMessagesEvents` framing, `createEventConverter` subset)

/// Upstream gateway normalization (`normalizeRadiusGatewayUrl`): scheme
/// added when missing, trailing slash stripped.
fn radius_gateway() -> String {
    let raw = std::env::var("RADIUS_GATEWAY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GATEWAY.to_string());
    let raw = raw.trim();
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// One content slot of the partial assistant message (upstream
/// `partial.content`): text or tool call, keyed by the wire `contentIndex`.
#[derive(Debug)]
enum Slot {
    Text(String),
    Tool {
        id: String,
        name: String,
        args: String,
        done: bool,
    },
}

async fn stream_pi_messages(
    gateway: &str,
    model_id: &str,
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
    // pi-messages passes the pi conversation through: `context` is the
    // transcript, `options` is empty in v0 (documented).
    let body = json!({
        "model": model_id,
        "context": { "messages": messages },
        "options": {},
    });
    let mut reqb = client
        .post(format!("{gateway}/v1/messages"))
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .json(&body);
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
        // Upstream error body: `{error: {message, code}}` when present.
        let (message, code) = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| {
                let e = v.get("error")?;
                let message = e.get("message").and_then(Value::as_str).map(str::to_string);
                let code = e.get("code").and_then(Value::as_str).map(str::to_string);
                Some((message, code))
            })
            .unwrap_or((None, None));
        let detail = message.unwrap_or_else(|| body.chars().take(160).collect());
        return fail(format!(
            "HTTP {status}: {}{}",
            detail,
            code.map(|c| format!(" ({c})")).unwrap_or_default()
        ));
    }
    let mut slots: HashMap<u64, Slot> = HashMap::new();
    let mut usage = json!({"input": 0, "output": 0, "cache_read": 0, "cache_write": 0});
    let mut saw_terminal = false;

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return fail(format!("stream error: {e}")),
        };
        buf.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
        while let Some(pos) = buf.find("\n\n") {
            let frame = buf[..pos].to_string();
            buf.drain(..pos + 2);
            let Some(data) = frame
                .lines()
                .find(|l| l.starts_with("data:"))
                .map(|l| l["data:".len()..].trim().to_string())
            else {
                continue;
            };
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            let etype = ev.get("type").and_then(Value::as_str).unwrap_or_default();
            let idx = ev.get("contentIndex").and_then(Value::as_u64);
            match etype {
                "start" => {}
                "text_start" => {
                    if let Some(i) = idx {
                        slots.entry(i).or_insert_with(|| Slot::Text(String::new()));
                    }
                }
                "text_delta" => {
                    if let Some(i) = idx {
                        let d = ev.get("delta").and_then(Value::as_str).unwrap_or_default();
                        match slots.entry(i) {
                            std::collections::hash_map::Entry::Occupied(mut o) => {
                                if let Slot::Text(t) = o.get_mut() {
                                    t.push_str(d);
                                    let _ = tx.send(json!({"type": "delta", "delta": d}));
                                }
                            }
                            std::collections::hash_map::Entry::Vacant(v) => {
                                v.insert(Slot::Text(d.to_string()));
                                let _ = tx.send(json!({"type": "delta", "delta": d}));
                            }
                        }
                    }
                }
                "text_end" => {
                    // The event's content is authoritative (upstream).
                    if let Some(i) = idx {
                        if let Some(c) = ev.get("content").and_then(Value::as_str) {
                            slots.insert(i, Slot::Text(c.to_string()));
                        }
                    }
                }
                "toolcall_start" => {
                    if let Some(i) = idx {
                        slots.insert(
                            i,
                            Slot::Tool {
                                id: ev
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: ev
                                    .get("toolName")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                args: String::new(),
                                done: false,
                            },
                        );
                    }
                }
                "toolcall_delta" => {
                    if let Some(i) = idx {
                        if let Some(Slot::Tool {
                            args, done: false, ..
                        }) = slots.get_mut(&i)
                        {
                            args.push_str(
                                ev.get("delta").and_then(Value::as_str).unwrap_or_default(),
                            );
                        }
                    }
                }
                "toolcall_end" => {
                    if let Some(i) = idx {
                        let tc = &ev["toolCall"];
                        slots.insert(
                            i,
                            Slot::Tool {
                                id: tc
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: tc
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                args: tc
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or(json!({}))
                                    .to_string(),
                                done: true,
                            },
                        );
                    }
                }
                "thinking_start" | "thinking_delta" | "thinking_end" => {
                    // Not ported (v0: no thinking support in the ABI).
                }
                "done" => {
                    saw_terminal = true;
                    if let Some(u) = ev.get("usage") {
                        // pi-messages usage is camelCase (upstream shape).
                        usage["input"] = json!(u.get("input").and_then(Value::as_u64).unwrap_or(0));
                        usage["output"] =
                            json!(u.get("output").and_then(Value::as_u64).unwrap_or(0));
                        usage["cache_read"] =
                            json!(u.get("cacheRead").and_then(Value::as_u64).unwrap_or(0));
                        usage["cache_write"] =
                            json!(u.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0));
                    }
                }
                "error" => {
                    let msg = ev
                        .get("errorMessage")
                        .and_then(Value::as_str)
                        .unwrap_or("pi-messages stream error");
                    return fail(msg.to_string());
                }
                _ => {}
            }
        }
    }
    if !saw_terminal {
        return fail("pi-messages stream ended without a terminal event".to_string());
    }
    let mut entries: Vec<(u64, &Slot)> = slots.iter().map(|(k, v)| (*k, v)).collect();
    entries.sort_by_key(|(i, _)| *i);
    let text: String = entries
        .iter()
        .filter_map(|(_, s)| match s {
            Slot::Text(t) => Some(t.as_str()),
            Slot::Tool { .. } => None,
        })
        .collect();
    let tool_calls: Vec<Value> = entries
        .iter()
        .filter_map(|(_, s)| match s {
            Slot::Tool {
                id,
                name,
                args,
                done,
            } if *done => {
                let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                Some(json!({ "id": id, "name": name, "input": input }))
            }
            _ => None,
        })
        .collect();
    let _ = tx.send(json!({
        "type": "done",
        "turn": {
            "text": if text.is_empty() { Value::Null } else { Value::String(text) },
            "tool_calls": tool_calls,
            "usage": usage,
        },
    }));
}

// ---------------------------------------------------------------------------
// OAuth: device-code login + refresh (upstream `auth/oauth/radius.ts`)

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

/// Upstream `trustedHttpUrl`: trusted http(s) (self-hosted dev gateways
/// may be plain http — the radius-specific relaxation).
fn trusted_http_url(raw: &str) -> Result<String, String> {
    let ok = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .filter(|rest| !rest.is_empty())
        .is_some();
    if ok {
        Ok(raw.to_string())
    } else {
        Err("Untrusted verification_uri in Radius OAuth device authorization response".to_string())
    }
}

async fn post_form(
    gateway: &str,
    path: &str,
    fields: &[(&str, &str)],
) -> Result<(u16, Value), String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .post(format!("{gateway}{path}"))
        .form(fields)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Radius OAuth request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let body = if body.is_object() { body } else { json!({}) };
    Ok((status, body))
}

/// Upstream `requestOAuthToken` response shape: access + refresh +
/// expires_in (all required); `scope` is carried through when present
/// (upstream credential field).
fn credentials_from_token_response(body: &Value) -> Result<Value, String> {
    let access = body
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid Radius OAuth response field: access_token".to_string())?;
    let refresh = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Invalid Radius OAuth response field: refresh_token".to_string())?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or_else(|| "Invalid Radius OAuth response field: expires_in".to_string())?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let mut cred = json!({
        "type": "oauth",
        "access": access,
        "refresh": refresh,
        "expires": now_ms + expires_in as i64 * 1000,
    });
    if let Some(scope) = body.get("scope").and_then(Value::as_str) {
        cred["scope"] = json!(scope);
    }
    Ok(cred)
}

async fn device_authorization(gateway: &str) -> Result<(Value, Option<String>), String> {
    let (status, body) = post_form(
        gateway,
        "/v1/oauth/device",
        &[("client_id", OAUTH_CLIENT_ID), ("scope", OAUTH_SCOPE)],
    )
    .await
    .map_err(|e| format!("Radius OAuth device authorization request failed: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(request_failure("device authorization", status, &body));
    }
    let device_code = body
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "Radius OAuth device authorization response is missing required fields".to_string()
        })?;
    // The user code is surfaced in the pending status (`code` field — the
    // ABI v1 pending payload extension, E4S2); the core prints it.
    let user_code = body
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "Radius OAuth device authorization response is missing required fields".to_string()
        })?;
    let verification_uri = trusted_http_url(
        body.get("verification_uri")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Radius OAuth device authorization response is missing required fields".to_string()
            })?,
    )?;
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|v| *v > 0)
        .ok_or_else(|| {
            "Radius OAuth device authorization response is missing required fields".to_string()
        })?;
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
        Some(verification_uri),
    ))
}

fn request_failure(action: &str, status: u16, body: &Value) -> String {
    // Upstream `readOAuthResponseError` message shape.
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
        format!("Radius OAuth {action} failed (HTTP {status})")
    } else {
        format!(
            "Radius OAuth {action} failed (HTTP {status}): {}",
            detail.join(" ")
        )
    }
}

/// The device-code poll loop (upstream `loginWithDeviceCode` via the
/// shared `pollOAuthDeviceCodeFlow`): wait before the first poll, honor
/// `slow_down` (server interval or +5 s), stop at the deadline.
async fn poll_device_tokens(gateway: &str, device: &Value) -> Result<Value, String> {
    let deadline = std::time::Instant::now()
        + Duration::from_secs(device["expires_in"].as_u64().unwrap_or(600));
    let mut interval = Duration::from_secs(
        device["interval"]
            .as_u64()
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
        let (status, body) = post_form(
            gateway,
            "/v1/oauth/token",
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", OAUTH_CLIENT_ID),
                ("device_code", device["device_code"].as_str().unwrap_or("")),
            ],
        )
        .await
        .map_err(|e| format!("Radius OAuth device token polling request failed: {e}"))?;
        if (200..299).contains(&status) {
            return credentials_from_token_response(&body)
                .map_err(|e| format!("Radius OAuth device token parse failed: {e}"));
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
            Some("expired_token") => {
                return Err("Device authorization expired.".to_string());
            }
            Some("access_denied") => {
                return Err("Device authorization was denied.".to_string());
            }
            _ => {
                return Err(request_failure("device token polling", status, &body));
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

/// One refresh (upstream `createRadiusOAuth.refresh`):
/// `grant_type=refresh_token`. Synchronous (the core's refresh call).
async fn refresh_tokens(gateway: &str, refresh_token: &str) -> Result<Value, String> {
    let (status, body) = post_form(
        gateway,
        "/v1/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", OAUTH_CLIENT_ID),
            ("refresh_token", refresh_token),
        ],
    )
    .await
    .map_err(|e| format!("Radius OAuth token refresh request failed: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(request_failure("token refresh", status, &body));
    }
    credentials_from_token_response(&body)
        .map_err(|e| format!("Radius OAuth token refresh failed: {e}"))
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
        let gateway = radius_gateway();
        let res = match refresh_token {
            Some(r) => runtime().block_on(refresh_tokens(&gateway, r)),
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
    let gateway = radius_gateway();
    let (device, url) = match runtime().block_on(device_authorization(&gateway)) {
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
        let gateway = gateway.clone();
        async move {
            match poll_device_tokens(&gateway, &device).await {
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
