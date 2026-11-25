//! pi provider plugin — OpenAI-compatible sample (E4S1/WP-1501).
//!
//! Reference implementation of the pi provider plugin ABI v1
//! (`docs/provider-plugin-abi.md`) and the e2e test fixture
//! (`tests/provider_plugin.rs`).
//!
//! The core owns the login UX and the `auth.json` credential store — this
//! plugin never touches either: credentials arrive in the stream request
//! (`credential` field) and the mock login handshake returns the credential
//! material to the core, which stores it.
//!
//! - Provider `sample-compat`, one model per variant (the e2e uses the
//!   variant to verify the user-dir override):
//!   `PI_SAMPLE_VARIANT=bundled` (default) → `bundled-model`;
//!   `PI_SAMPLE_VARIANT=user` → `user-model`.
//! - Stream: `POST {base}/chat/completions` (`stream: true`), SSE deltas.
//!   Base URL: the stream request's `model.baseUrl`, else
//!   `PI_SAMPLE_BASE_URL`, else `https://api.openai.com/v1`.
//! - Login (mock): `auth_start` opens a one-shot loopback listener on
//!   127.0.0.1; a GET on the returned URL completes the login and
//!   `auth_status` reports `done` + a mock `api_key` credential.
//!
//! Build (test fixture — not a default workspace member):
//! `cargo build -p pi-sample-compat`.

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

const PROVIDER_NAME: &str = "sample-compat";
const ABI_VERSION: u32 = 1;

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
// vtable call — the returned pointer is valid until then, so one buffer per
// calling thread keeps the plugin leak-free; it is always NUL-terminated).
// Thread-local: concurrent calls from different threads (parallel core
// blocking-pool tasks, parallel tests) must not clobber each other's
// returned strings.

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

static NAME_BYTES: &[u8] = b"sample-compat\0";

extern "C" fn name() -> *const u8 {
    NAME_BYTES.as_ptr()
}

extern "C" fn list_models() -> *const u8 {
    let variant = std::env::var("PI_SAMPLE_VARIANT").unwrap_or_else(|_| "bundled".into());
    let (id, label) = if variant == "user" {
        ("user-model", "Sample User Model")
    } else {
        ("bundled-model", "Sample Bundled Model")
    };
    // Registry-shaped entry (camelCase, the core's Model schema). The core
    // sets `provider` from the vtable name; `api` is informational.
    let arr = json!([{
        "id": id,
        "name": label,
        "api": "openai-completions",
        "provider": PROVIDER_NAME,
        "input": ["text"],
        "contextWindow": 4096,
        "maxTokens": 512,
    }]);
    out(&arr.to_string())
}

// ---------------------------------------------------------------------------
// Streaming (the plugin's own runtime; the vtable calls block per event)

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
            .expect("sample plugin: tokio runtime")
    })
}

/// `{"handle": ...}` on success, `{"error": ...}` on a request-level failure
/// (wire failures surface as stream events, not open failures).
extern "C" fn stream_open(request: *const u8) -> *const u8 {
    let req: Value = match serde_json::from_str(&read_cstr(request)) {
        Ok(v) => v,
        Err(e) => return err(&format!("stream request is not valid JSON: {e}")),
    };
    let model = req
        .get("model")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let model_id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let messages = req.get("messages").cloned().unwrap_or(Value::Array(vec![]));
    let base = model
        .get("baseUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            std::env::var("PI_SAMPLE_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "https://api.openai.com/v1".into());
    // The core resolved the credential (auth.json / login); the plugin only
    // consumes it here — it is never stored plugin-side.
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

    let rt = runtime();
    let tx_outer = tx.clone();
    rt.spawn({
        let inner = rt.spawn(async move {
            let fail = |msg: String| {
                let _ = tx.send(json!({"type": "error", "error": msg}));
            };
            let body = json!({ "model": model_id, "messages": messages, "stream": true });
            let client = match reqwest::Client::builder().build() {
                Ok(c) => c,
                Err(e) => return fail(format!("http client: {e}")),
            };
            let mut reqb = client
                .post(format!("{}/chat/completions", base.trim_end_matches('/')))
                .json(&body);
            if let Some(t) = &token {
                reqb = reqb.bearer_auth(t);
            }
            let resp = match reqb.send().await {
                Ok(r) => r,
                Err(e) => return fail(format!("request failed: {e}")),
            };
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return fail(format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()));
            }
            let mut text = String::new();
            let mut input = 0u64;
            let mut output = 0u64;
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
                    if payload == "[DONE]" {
                        break;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(payload) else {
                        continue;
                    };
                    if let Some(usage) = v.get("usage") {
                        input = usage
                            .get("prompt_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(input);
                        output = usage
                            .get("completion_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(output);
                    }
                    if let Some(d) = v
                        .pointer("/choices/0/delta/content")
                        .and_then(Value::as_str)
                    {
                        text.push_str(d);
                        let _ = tx.send(json!({"type": "delta", "delta": d}));
                    }
                }
            }
            let text_value = if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            };
            let _ = tx.send(json!({
                "type": "done",
                "turn": {
                    "text": text_value,
                    "tool_calls": [],
                    "usage": { "input": input, "output": output, "cache_read": 0, "cache_write": 0 },
                },
            }));
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
// Mock login (the core drives the UX and stores the credential)

struct AuthState {
    url: Option<String>,
    credential: Option<Value>,
    failed: Option<String>,
}

static AUTH: Mutex<AuthState> = Mutex::new(AuthState {
    url: None,
    credential: None,
    failed: None,
});
static LOGIN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Starts the mock handshake: a one-shot loopback listener; a GET on the
/// returned URL completes the login. Returns the first status.
extern "C" fn auth_start(_request: *const u8) -> *const u8 {
    let mut g = AUTH.lock().unwrap();
    *g = AuthState {
        url: None,
        credential: None,
        failed: None,
    };
    let (listener, port) = match TcpListener::bind("127.0.0.1:0").and_then(|l| {
        l.local_addr()
            .map(|a| (l, a.port()))
            .map_err(|e| std::io::Error::other(e.to_string()))
    }) {
        Ok(x) => x,
        Err(e) => {
            g.failed = Some(format!("loopback bind failed: {e}"));
            let msg = g.failed.clone().unwrap_or_default();
            return out(&json!({"state": "failed", "error": msg}).to_string());
        }
    };
    std::thread::spawn(move || {
        // One-shot mock: any request completes the sign-in.
        if let Ok((mut conn, _)) = listener.accept() {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(60)));
            let mut buf = [0u8; 1024];
            let _ = conn.read(&mut buf); // the request (ignored — mock)
            let body = b"ok";
            let _ = conn.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\n",
            );
            let _ = conn.write_all(body);
            let _ = conn.flush();
        }
        let key = format!(
            "mock-key-{}",
            LOGIN_COUNTER.fetch_add(1, Ordering::SeqCst) + 1
        );
        let mut g = AUTH.lock().unwrap();
        g.credential = Some(json!({ "type": "api_key", "key": key }));
    });
    let url = format!("http://127.0.0.1:{port}/auth/callback");
    g.url = Some(url.clone());
    out(&json!({"state": "pending", "method": "loopback", "url": url}).to_string())
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
    let url = g
        .url
        .as_ref()
        .map(|u| json!({"state": "pending", "url": u}))
        .unwrap_or_else(|| json!({"state": "pending"}));
    out(&url.to_string())
}

extern "C" fn dispose() {
    STREAMS.lock().unwrap().clear();
    *AUTH.lock().unwrap() = AuthState {
        url: None,
        credential: None,
        failed: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn vt() -> &'static PiProviderVtable {
        &VT
    }

    #[test]
    fn vtable_shape_and_name() {
        let vt = vt();
        assert_eq!((vt.abi_version)(), 1);
        assert_eq!((vt.name)(), NAME_BYTES.as_ptr());
        let models: Value = serde_json::from_str(
            unsafe { CStr::from_ptr((vt.list_models)() as *const _) }
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let m = &models.as_array().unwrap()[0];
        assert_eq!(m["id"], "bundled-model");
        assert_eq!(m["provider"], PROVIDER_NAME);
    }

    #[test]
    fn login_handshake_reports_done_after_callback() {
        let vt = vt();
        let started = unsafe { CStr::from_ptr((vt.auth_start)(std::ptr::null()) as *const _) };
        let st: Value = serde_json::from_str(started.to_str().unwrap()).unwrap();
        assert_eq!(st["state"], "pending");
        let url = st["url"].as_str().unwrap().to_string();
        let (hostport, _) = url
            .strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let (host, port) = hostport.rsplit_once(':').unwrap();
        let mut c = TcpStream::connect((host, port.parse().unwrap())).unwrap();
        let _ = c.write_all(b"GET /auth/callback HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
        let mut b = [0u8; 512];
        let _ = c.read(&mut b);
        // The listener thread sets the credential after answering.
        let mut st = Value::Null;
        for _ in 0..100 {
            let s = unsafe { CStr::from_ptr((vt.auth_status)(std::ptr::null()) as *const _) };
            st = serde_json::from_str(s.to_str().unwrap()).unwrap();
            if st["state"] == "done" {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(st["state"], "done");
        assert_eq!(st["credential"]["type"], "api_key");
        assert!(st["credential"]["key"]
            .as_str()
            .unwrap()
            .starts_with("mock-key-"));
    }
}
