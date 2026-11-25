//! E4S2/WP-1502 (HARD acceptance): the four remaining oauth builtins —
//! radius, github-copilot, xai, meta — run through the CORE as provider
//! plugins, with the sidecar DEGRADED (spawn failure — no Node needed).
//! Per provider:
//! - RED: without the plugin present the provider is a typed failure
//!   (the E4S1/E3S5 rejection messages) — the plugin is what makes it work.
//! - LOGIN: `pi-rust auth login <p> --no-browser` runs the per-provider
//!   device-code flow against a LOCAL MOCK oauth endpoint (WP-1404 mock
//!   pattern, in-process Rust; the core drives it via the plugin's
//!   `auth_start`/`auth_status`); the token lands in auth.json in the
//!   upstream credential shape.
//! - STREAM: a run with a pre-seeded EXPIRED credential refreshes through
//!   the plugin (core's 5-minute validity margin), writes the ROTATED
//!   credential back, and streams a mock SSE response — the transcript
//!   carrying the mock text proves the wire never crossed the bridge.
//!
//! All traffic is local (mock oauth + mock SSE); no real network.
//!
//! Parallel-test safety: one static lock serializes every test in this
//! file — they share the BUNDLED plugin dir (next to the test binary)
//! and build into a shared cargo target dir (WP-1405 incident pattern;
//! the KIMI_ENV_LOCK generalization — process-global state, not env vars:
//! per-run env is passed to the child, never set process-wide).

use serde_json::{json, Value};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Shared lock + bundled plugin dir

/// Serializes all tests in this file (shared bundled plugin dir + shared
/// cargo build target).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn pi_bin() -> String {
    env!("CARGO_BIN_EXE_pi-rust").to_string()
}

fn bundled_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_BIN_EXE_pi-rust"))
        .parent()
        .expect("test binary dir")
        .join("providers");
    std::fs::create_dir_all(&dir).expect("bundled plugin dir");
    dir
}

/// The platform cdylib extension (the E4S1 lesson: `.so` is Linux-only —
/// macOS builds `.dylib`, Windows `.dll`).
fn plugin_ext() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    }
}

fn remove_bundled(crate_name: &str) {
    let lib = crate_name.replace('-', "_");
    for ext in ["so", "dylib", "dll"] {
        let _ = std::fs::remove_file(bundled_dir().join(format!("libpi_provider_{lib}.{ext}")));
    }
}

/// Builds the plugin crate on demand (shared target dir — the four crates
/// share the compiled dependencies) and returns the cdylib path.
fn build_plugin(crate_name: &str) -> std::path::PathBuf {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Fixed shared target dir (persists across runs — a build cache like
    // the main target/; the four crates share the compiled dependencies).
    let shared_target = std::env::temp_dir().join("e4s2-plugins-target");
    let manifest = repo.join("plugins").join(crate_name).join("Cargo.toml");
    let out = Command::new("cargo")
        .env("CARGO_TARGET_DIR", &shared_target)
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("running cargo build for the plugin crate");
    assert!(
        out.status.success(),
        "building plugins/{crate_name} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lib = crate_name.replace('-', "_");
    let so = shared_target
        .join("debug")
        .join(format!("libpi_provider_{lib}.{}", plugin_ext()));
    assert!(so.exists(), "missing cdylib {so:?}");
    so
}

/// Copies the plugin cdylib into the bundled dir (the E4S1 layout:
/// <exe dir>/providers/ — where a release ships them) and returns the
/// destination path.
fn install_bundled(crate_name: &str) -> std::path::PathBuf {
    let so = build_plugin(crate_name);
    let lib = crate_name.replace('-', "_");
    let dest = bundled_dir().join(format!("libpi_provider_{lib}.{}", plugin_ext()));
    std::fs::copy(&so, &dest).expect("copying the plugin into the bundled dir");
    dest
}

/// Runs pi-rust with the sidecar DEGRADED (the node binary does not
/// exist) + the given provider env, against a fresh agent dir.
fn run_pi(args: &[&str], agent_dir: &std::path::Path, env: &[(&str, &str)]) -> Output {
    Command::new(pi_bin())
        .args(args)
        .arg("--agent-dir")
        .arg(agent_dir)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .envs(env.iter().copied())
        .output()
        .expect("running pi-rust")
}

fn read_auth(agent_dir: &std::path::Path) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(agent_dir.join("auth.json"))
            .unwrap_or_else(|_| panic!("auth.json missing in {agent_dir:?}")),
    )
    .expect("parsing auth.json")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The session transcript (the `--session-file` JSONL) as one string.
fn session_text(session_file: &std::path::Path) -> String {
    std::fs::read_to_string(session_file)
        .unwrap_or_else(|_| panic!("session file missing: {}", session_file.display()))
}

// ---------------------------------------------------------------------------
// In-process HTTP mock (WP-1404 mock pattern, Rust, per-provider routes)

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    headers: String,
    body: String,
}

impl Req {
    /// A lower-cased header value (e.g. "authorization").
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().to_string())
        })
    }
}

/// A single-threaded mock HTTP server: records every request, answers via
/// the handler (status, content-type, body; `Connection: close`).
struct Mock {
    base: String,
    reqs: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    fn requests(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }
    /// Requests to a path prefix, in order.
    fn to(&self, path: &str) -> Vec<Req> {
        self.requests()
            .into_iter()
            .filter(|r| r.path.starts_with(path))
            .collect()
    }
}

type Handler = Box<dyn FnMut(&Req) -> (u16, &str, String) + Send>;

fn start_mock(handler: Handler) -> Mock {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let reqs: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
    let reqs2 = reqs.clone();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut handler = handler;
        loop {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            // Read the full request: headers, then `content-length` bytes.
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 8192];
            let head_end = loop {
                let n = match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break None,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(head_end) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let mut content_length = 0usize;
            for line in head.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            while buf.len() < head_end + content_length {
                let n = match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&tmp[..n]);
            }
            let head_lines: Vec<&str> = head.lines().collect();
            let first = head_lines.first().copied().unwrap_or_default();
            let parts: Vec<&str> = first.split_whitespace().collect();
            let (method, path) = match parts.as_slice() {
                [m, p, ..] => (m.to_string(), p.to_string()),
                _ => (first.to_string(), "/".to_string()),
            };
            let req = Req {
                method: method.to_string(),
                path: path.to_string(),
                headers: head_lines[1..].join("\n"),
                body: String::from_utf8_lossy(&buf[head_end..]).to_string(),
            };
            reqs2.lock().unwrap().push(req.clone());
            let (status, content_type, body) = handler(&req);
            let reason = if (200..300).contains(&status) {
                "OK"
            } else if status == 400 {
                "Bad Request"
            } else {
                "Internal Server Error"
            };
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });
    Mock { base, reqs }
}

/// A body form-field (`grant_type=...&device_code=...`).
fn form_field(body: &str, field: &str) -> Option<String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == field)
        .map(|(_, v)| v.to_string())
}

// The SSE response bodies (one full body per stream — the client's
// stream reader gets the events in order, then EOF).

/// openai-responses SSE: a text-only turn (upstream event names).
fn sse_responses(text: &str) -> String {
    let item = json!({"type": "message", "id": "msg-1", "role": "assistant", "content": [{"type": "output_text", "text": text}]});
    format!(
        "data: {{\"type\":\"response.created\"}}\n\n\
         data: {{\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{item}}}\n\n\
         data: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"{}\"}}\n\n\
         data: {{\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"{}\"}}\n\n\
         data: {{\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{item}}}\n\n\
         data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-1\",\"status\":\"completed\",\"usage\":{{\"input_tokens\":12,\"output_tokens\":7}}}}}}\n\n",
        text.chars().take(3).collect::<String>(),
        text.chars().skip(3).collect::<String>()
    )
}

/// openai-completions SSE: a text-only turn + a final usage chunk.
fn sse_completions(text: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"content\":\"{}\"}}}}]}}\n\n\
         data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":4}}}}\n\n\
         data: [DONE]\n\n",
        text.replace('"', "\\\"")
    )
}

/// anthropic-messages SSE: a text-only turn (upstream event names).
fn sse_anthropic(text: &str) -> String {
    let h1: String = text.chars().take(3).collect();
    let h2: String = text.chars().skip(3).collect();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"m1\",\"usage\":{{\"input_tokens\":8,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":0}}}}}}\n\n\
         event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{h1}\"}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{h2}\"}}}}\n\n\
         event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":5}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

/// pi-messages SSE (radius): a text-only turn (upstream event names).
fn sse_pi_messages(text: &str) -> String {
    format!(
        "data: {{\"type\":\"start\"}}\n\n\
         data: {{\"type\":\"text_start\",\"contentIndex\":0}}\n\n\
         data: {{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"{}\"}}\n\n\
         data: {{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"{}\"}}\n\n\
         data: {{\"type\":\"text_end\",\"contentIndex\":0,\"content\":\"{}\"}}\n\n\
         data: {{\"type\":\"done\",\"reason\":\"stop\",\"usage\":{{\"input\":5,\"output\":6,\"cacheRead\":0,\"cacheWrite\":0}}}}\n\n",
        text.replace('"', "\\\""),
        "",
        text.replace('"', "\\\"")
    )
}

// ---------------------------------------------------------------------------
// RED: without the plugins the four providers are typed failures

/// E4S2 TDD RED (permanent): with none of the four plugin cdylibs present
/// (bundled or user), every provider is a typed rejection — the plugins
/// are exactly what makes them work.
#[test]
fn e4s2_red_without_plugins_the_four_are_unavailable() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for crate_name in ["radius", "github-copilot", "xai", "meta"] {
        remove_bundled(crate_name);
    }
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s2-red-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let cases = [
        ("xai", "grok-4.7", "not supported in v0"),
        ("meta", "muse-spark-1.2", "not supported in v0"),
        ("github-copilot", "gpt-5.6-sol", "not supported in v0"),
        (
            "radius",
            "balanced",
            "not a built-in or models.json provider",
        ),
    ];
    for (provider, model, expected) in cases {
        let agent = tmp.join(format!("agent-{provider}"));
        std::fs::create_dir_all(&agent).unwrap();
        let out = run_pi(
            &["--provider", provider, "--model", model, "--prompt", "hi"],
            &agent,
            &[],
        );
        assert!(
            out.status.code() != Some(0),
            "{provider} must be unavailable without its plugin"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stderr.contains(expected) || stdout.contains(expected),
            "{provider}: expected {expected:?} in\n{stdout}\n{stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// xai

#[test]
fn e4s2_xai_login_stream_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    remove_bundled("xai");
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s2-xai-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Mock oauth + wire (all local).
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let polls2 = polls.clone();
    let mock = start_mock(Box::new(move |req: &Req| {
        let json = "application/json";
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/oauth2/device/code") => (
                200,
                json,
                json!({
                    "device_code": "xai-device-1",
                    "user_code": "XAI-MOCK",
                    "verification_uri": "https://auth.x.ai/activate",
                    "interval": 1,
                    "expires_in": 120,
                })
                .to_string(),
            ),
            ("POST", "/oauth2/token") => {
                let grant = form_field(&req.body, "grant_type").unwrap_or_default();
                if grant.contains("device_code") {
                    let n = polls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        (
                            400,
                            json,
                            json!({"error": "authorization_pending"}).to_string(),
                        )
                    } else {
                        (
                            200,
                            json,
                            json!({
                                "access_token": "xai-access-1",
                                "refresh_token": "xai-refresh-1",
                                "expires_in": 3600,
                            })
                            .to_string(),
                        )
                    }
                } else if grant == "refresh_token" {
                    (
                        200,
                        json,
                        json!({
                            "access_token": "xai-access-2",
                            "refresh_token": "xai-refresh-2",
                            "expires_in": 3600,
                        })
                        .to_string(),
                    )
                } else {
                    (
                        400,
                        json,
                        json!({"error": "unsupported_grant_type"}).to_string(),
                    )
                }
            }
            ("POST", "/responses") => (
                200,
                "text/event-stream",
                sse_responses("XAI hello from the plugin"),
            ),
            _ => (404, json, json!({"error": "not found"}).to_string()),
        }
    }));

    // RED: without the plugin, a typed v0 rejection.
    {
        let agent = tmp.join("agent-red");
        std::fs::create_dir_all(&agent).unwrap();
        let out = run_pi(
            &["--provider", "xai", "--model", "grok-4.7", "--prompt", "hi"],
            &agent,
            &[],
        );
        assert_ne!(out.status.code(), Some(0), "RED must fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("not supported in v0"), "RED: {stderr}");
    }

    install_bundled("xai");

    // LOGIN: the device-code flow against the mock (the core drives the
    // plugin; --no-browser is headless-safe).
    let agent_login = tmp.join("agent-login");
    std::fs::create_dir_all(&agent_login).unwrap();
    let out = Command::new(pi_bin())
        .args(["auth", "login", "xai", "--no-browser"])
        .env("PI_CODING_AGENT_DIR", &agent_login)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("XAI_OAUTH_HOST", &mock.base)
        .env("XAI_BASE_URL", &mock.base)
        .output()
        .expect("running pi-rust auth login");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "login\n{stdout}\n{stderr}");
    assert!(stdout.contains("Logged in to xai"), "login\n{stdout}");
    // The user code was surfaced (ABI v1 pending `code`).
    assert!(
        stdout.contains("XAI-MOCK"),
        "the device-code user code must be printed\n{stdout}"
    );
    let cred = &read_auth(&agent_login)["xai"];
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("xai-access-1"));
    assert_eq!(cred["refresh"], json!("xai-refresh-1"));
    assert!(cred["expires"].as_i64().unwrap_or(0) > now_ms());
    // The device flow went to the mock with the upstream client id.
    let dev = mock.to("/oauth2/device/code");
    assert!(!dev.is_empty(), "device authorization was not requested");
    assert!(dev[0]
        .body
        .contains("client_id=b1a00492-073a-47ea-816f-4c329264a828"));
    // STREAM + REFRESH: a fresh agent dir with an EXPIRED credential — the
    // core refreshes through the plugin (rotated write-back) and streams.
    let agent_run = tmp.join("agent-run");
    std::fs::create_dir_all(&agent_run).unwrap();
    std::fs::write(
        agent_run.join("auth.json"),
        r#"{"xai":{"type":"oauth","access":"xai-old","refresh":"xai-refresh-1","expires":1}}"#,
    )
    .unwrap();
    let session = tmp.join("session-xai.jsonl");
    let out = Command::new(pi_bin())
        .arg("--provider")
        .arg("xai")
        .arg("--model")
        .arg("grok-4.7")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_run)
        .arg("--session-file")
        .arg(&session)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("XAI_OAUTH_HOST", &mock.base)
        .env("XAI_BASE_URL", &mock.base)
        .output()
        .expect("running pi-rust xai");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "run\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path\n{stderr}"
    );
    // The transcript carries the mock text (the call never crossed a bridge).
    let dump = session_text(&session);
    assert!(
        dump.contains("XAI hello from the plugin"),
        "the plugin must have served the stream\n{dump}"
    );
    // The refresh used the stored refresh token (grant_type + token).
    let token_reqs = mock.to("/oauth2/token");
    let refresh_req = token_reqs
        .iter()
        .find(|r| form_field(&r.body, "grant_type").as_deref() == Some("refresh_token"))
        .expect("no refresh request");
    assert_eq!(
        form_field(&refresh_req.body, "refresh_token").as_deref(),
        Some("xai-refresh-1"),
        "the refresh must use the stored refresh token"
    );
    // The rotated credential is written back (upstream shape).
    let cred = &read_auth(&agent_run)["xai"];
    assert_eq!(cred["access"], json!("xai-access-2"), "rotated access");
    assert_eq!(cred["refresh"], json!("xai-refresh-2"), "rotated refresh");
    assert!(cred["expires"].as_i64().unwrap_or(0) > now_ms());
    // The stream used the ROTATED token.
    let stream_req = mock.to("/responses");
    assert!(!stream_req.is_empty());
    assert_eq!(
        stream_req[0].header("authorization").as_deref(),
        Some("Bearer xai-access-2"),
        "the stream must use the rotated access token"
    );
    // Redaction: no token material in the process output.
    for secret in [
        "xai-access-1",
        "xai-access-2",
        "xai-refresh-1",
        "xai-refresh-2",
    ] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "token material {secret} leaked into the process output"
        );
    }
    remove_bundled("xai");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// meta

#[test]
fn e4s2_meta_login_stream_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    remove_bundled("meta");
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s2-meta-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let polls2 = polls.clone();
    let mints2 = mints.clone();
    let mock = start_mock(Box::new(move |req: &Req| {
        let json = "application/json";
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/oidc/device/authorization/") => (
                200,
                json,
                json!({
                    "device_code": "meta-device-1",
                    "user_code": "META-MOCK",
                    "verification_uri": "https://auth.meta.com/activate",
                    "interval": 1,
                    "expires_in": 120,
                })
                .to_string(),
            ),
            ("POST", "/oidc/device/token/") => {
                let n = polls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    (
                        400,
                        json,
                        json!({"error": "authorization_pending"}).to_string(),
                    )
                } else {
                    (
                        200,
                        json,
                        json!({"access_token": "meta-identity-1"}).to_string(),
                    )
                }
            }
            // The key-mint: login mints key-1; the refresh re-mints key-2.
            ("POST", "/muse-code/key") => {
                let n = mints2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let key = if n == 0 { "meta-key-1" } else { "meta-key-2" };
                (200, json, json!({"api_key": key}).to_string())
            }
            ("POST", "/v1/responses") => (
                200,
                "text/event-stream",
                sse_responses("META hello from the plugin"),
            ),
            _ => (404, json, json!({"error": "not found"}).to_string()),
        }
    }));

    // RED
    {
        let agent = tmp.join("agent-red");
        std::fs::create_dir_all(&agent).unwrap();
        let out = run_pi(
            &[
                "--provider",
                "meta",
                "--model",
                "muse-spark-1.2",
                "--prompt",
                "hi",
            ],
            &agent,
            &[],
        );
        assert_ne!(out.status.code(), Some(0), "RED must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not supported in v0"),
            "RED: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    install_bundled("meta");

    // LOGIN: device flow + key-mint.
    let agent_login = tmp.join("agent-login");
    std::fs::create_dir_all(&agent_login).unwrap();
    let out = Command::new(pi_bin())
        .args(["auth", "login", "meta", "--no-browser"])
        .env("PI_CODING_AGENT_DIR", &agent_login)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("META_OAUTH_HOST", &mock.base)
        .env("META_API_HOST", &mock.base)
        .output()
        .expect("running pi-rust auth login");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "login\n{stdout}\n{stderr}");
    assert!(stdout.contains("Logged in to meta"), "login\n{stdout}");
    assert!(stdout.contains("META-MOCK"), "user code\n{stdout}");
    let cred = &read_auth(&agent_login)["meta"];
    // Upstream shape: the MINTED key is `access`, the IDENTITY token is
    // `refresh`, the key lives about a day.
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("meta-key-1"));
    assert_eq!(cred["refresh"], json!("meta-identity-1"));
    let expires = cred["expires"].as_i64().unwrap_or(0);
    assert!(
        expires > now_ms() && expires - now_ms() <= 24 * 60 * 60 * 1000,
        "the minted key must expire within a day: {cred}"
    );
    // The mint ran with the identity token.
    let mint = mock.to("/muse-code/key");
    assert!(!mint.is_empty());
    assert_eq!(
        mint[0].header("authorization").as_deref(),
        Some("Bearer meta-identity-1"),
        "the mint must use the identity token"
    );

    // STREAM + REFRESH: an expired stored credential → re-mint (key-2) +
    // stream with the new key.
    let agent_run = tmp.join("agent-run");
    std::fs::create_dir_all(&agent_run).unwrap();
    std::fs::write(
        agent_run.join("auth.json"),
        r#"{"meta":{"type":"oauth","access":"meta-key-old","refresh":"meta-identity-1","expires":1}}"#,
    )
    .unwrap();
    let session = tmp.join("session-meta.jsonl");
    let out = Command::new(pi_bin())
        .arg("--provider")
        .arg("meta")
        .arg("--model")
        .arg("muse-spark-1.2")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_run)
        .arg("--session-file")
        .arg(&session)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("META_OAUTH_HOST", &mock.base)
        .env("META_API_HOST", &mock.base)
        .output()
        .expect("running pi-rust meta");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "run\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "degraded\n{stderr}"
    );
    assert!(
        session_text(&session).contains("META hello from the plugin"),
        "the plugin must have served the stream\n{}",
        session_text(&session)
    );
    let cred = &read_auth(&agent_run)["meta"];
    assert_eq!(
        cred["access"],
        json!("meta-key-2"),
        "the re-minted key must be written back"
    );
    assert_eq!(
        cred["refresh"],
        json!("meta-identity-1"),
        "the identity token is unchanged"
    );
    let stream_req = mock.to("/v1/responses");
    assert_eq!(
        stream_req[0].header("authorization").as_deref(),
        Some("Bearer meta-key-2"),
        "the stream must use the re-minted key"
    );
    for secret in ["meta-key-1", "meta-key-2", "meta-identity-1"] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "token material {secret} leaked into the process output"
        );
    }
    remove_bundled("meta");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// radius

#[test]
fn e4s2_radius_login_stream_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    remove_bundled("radius");
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s2-radius-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let polls2 = polls.clone();
    let mock = start_mock(Box::new(move |req: &Req| {
        let json = "application/json";
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/v1/oauth/device") => (
                200,
                json,
                json!({
                    "device_code": "radius-device-1",
                    "user_code": "RADIUS-MOCK",
                    "verification_uri": "http://127.0.0.1/activate",
                    "interval": 1,
                    "expires_in": 120,
                })
                .to_string(),
            ),
            ("POST", "/v1/oauth/token") => {
                let grant = form_field(&req.body, "grant_type").unwrap_or_default();
                if grant.contains("device_code") {
                    let n = polls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n == 0 {
                        (
                            400,
                            json,
                            json!({"error": "authorization_pending"}).to_string(),
                        )
                    } else {
                        (
                            200,
                            json,
                            json!({
                                "access_token": "radius-access-1",
                                "refresh_token": "radius-refresh-1",
                                "expires_in": 3600,
                                "scope": "gateway offline_access",
                            })
                            .to_string(),
                        )
                    }
                } else if grant == "refresh_token" {
                    (
                        200,
                        json,
                        json!({
                            "access_token": "radius-access-2",
                            "refresh_token": "radius-refresh-2",
                            "expires_in": 3600,
                            "scope": "gateway offline_access",
                        })
                        .to_string(),
                    )
                } else {
                    (
                        400,
                        json,
                        json!({"error": "unsupported_grant_type"}).to_string(),
                    )
                }
            }
            ("POST", "/v1/messages") => (
                200,
                "text/event-stream",
                sse_pi_messages("Radius hello from the plugin"),
            ),
            _ => (404, json, json!({"error": "not found"}).to_string()),
        }
    }));

    // RED (radius is not a builtin at all — the sidecar-degraded message)
    {
        let agent = tmp.join("agent-red");
        std::fs::create_dir_all(&agent).unwrap();
        let out = run_pi(
            &[
                "--provider",
                "radius",
                "--model",
                "balanced",
                "--prompt",
                "hi",
            ],
            &agent,
            &[],
        );
        assert_ne!(out.status.code(), Some(0), "RED must fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("not a built-in or models.json provider"),
            "RED: {stderr}"
        );
    }
    install_bundled("radius");

    // LOGIN
    let agent_login = tmp.join("agent-login");
    std::fs::create_dir_all(&agent_login).unwrap();
    let out = Command::new(pi_bin())
        .args(["auth", "login", "radius", "--no-browser"])
        .env("PI_CODING_AGENT_DIR", &agent_login)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("RADIUS_GATEWAY", &mock.base)
        .output()
        .expect("running pi-rust auth login");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "login\n{stdout}\n{stderr}");
    assert!(stdout.contains("Logged in to radius"), "login\n{stdout}");
    assert!(stdout.contains("RADIUS-MOCK"), "user code\n{stdout}");
    let cred = &read_auth(&agent_login)["radius"];
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("radius-access-1"));
    assert_eq!(cred["refresh"], json!("radius-refresh-1"));
    assert!(cred["expires"].as_i64().unwrap_or(0) > now_ms());
    // Upstream scope is carried through the credential.
    assert_eq!(cred["scope"], json!("gateway offline_access"));
    let dev = mock.to("/v1/oauth/device");
    assert!(!dev.is_empty());
    assert!(dev[0].body.contains("client_id=pi-gateway"));

    // STREAM + REFRESH
    let agent_run = tmp.join("agent-run");
    std::fs::create_dir_all(&agent_run).unwrap();
    std::fs::write(
        agent_run.join("auth.json"),
        r#"{"radius":{"type":"oauth","access":"radius-old","refresh":"radius-refresh-1","expires":1}}"#,
    )
    .unwrap();
    let session = tmp.join("session-radius.jsonl");
    let out = Command::new(pi_bin())
        .arg("--provider")
        .arg("radius")
        .arg("--model")
        .arg("balanced")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_run)
        .arg("--session-file")
        .arg(&session)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("RADIUS_GATEWAY", &mock.base)
        .output()
        .expect("running pi-rust radius");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "run\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "degraded\n{stderr}"
    );
    assert!(
        session_text(&session).contains("Radius hello from the plugin"),
        "the plugin must have served the stream\n{}",
        session_text(&session)
    );
    // The pi-messages body passes the pi conversation through.
    let msg = mock.to("/v1/messages");
    assert!(!msg.is_empty());
    assert!(
        msg[0].body.contains("\"model\":\"balanced\""),
        "the pi-messages body must carry the model id\n{}",
        msg[0].body
    );
    let cred = &read_auth(&agent_run)["radius"];
    assert_eq!(cred["access"], json!("radius-access-2"), "rotated access");
    assert_eq!(
        cred["refresh"],
        json!("radius-refresh-2"),
        "rotated refresh"
    );
    assert_eq!(
        msg[0].header("authorization").as_deref(),
        Some("Bearer radius-access-2"),
        "the stream must use the rotated access token"
    );
    for secret in [
        "radius-access-1",
        "radius-access-2",
        "radius-refresh-1",
        "radius-refresh-2",
    ] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "token material {secret} leaked into the process output"
        );
    }
    remove_bundled("radius");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// github-copilot

#[test]
fn e4s2_github_copilot_login_stream_refresh() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    remove_bundled("github-copilot");
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s2-copilot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let exchanges = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let polls2 = polls.clone();
    let exchanges2 = exchanges.clone();
    let models_catalog = json!({
        "data": [
            {"id": "gpt-5.6-sol", "model_picker_enabled": true, "policy": {"state": "enabled"}, "capabilities": {"supports": {"tool_calls": true}}},
            {"id": "claude-sonnet-5", "model_picker_enabled": true, "policy": {"state": "enabled"}, "capabilities": {"supports": {"tool_calls": true}}},
            {"id": "gemini-3.8-flash", "model_picker_enabled": true, "policy": {"state": "enabled"}, "capabilities": {"supports": {"tool_calls": true}}},
        ],
    });
    let expires_at = now_ms() / 1000 + 3600;
    let mock = start_mock(Box::new(move |req: &Req| {
        let json = "application/json";
        match (req.method.as_str(), req.path.as_str()) {
            ("POST", "/login/device/code") => (
                200,
                json,
                json!({
                    "device_code": "cp-device-1",
                    "user_code": "CP-MOCK",
                    "verification_uri": "https://github.com/login/device",
                    "interval": 1,
                    "expires_in": 120,
                })
                .to_string(),
            ),
            ("POST", "/login/oauth/access_token") => {
                let n = polls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    (
                        401,
                        json,
                        json!({"error": "authorization_pending"}).to_string(),
                    )
                } else {
                    (
                        200,
                        json,
                        json!({"access_token": "gh-mock-token"}).to_string(),
                    )
                }
            }
            ("POST", "/copilot_internal/v2/token") => {
                let n = exchanges2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let token = if n == 0 {
                    "cp-mock-access-1"
                } else {
                    "cp-mock-access-2"
                };
                (
                    200,
                    json,
                    json!({"token": token, "expires_at": expires_at}).to_string(),
                )
            }
            ("GET", "/models") => (200, json, models_catalog.to_string()),
            ("POST", "/responses") => (
                200,
                "text/event-stream",
                sse_responses("COPILOT responses hello"),
            ),
            ("POST", "/chat/completions") => (
                200,
                "text/event-stream",
                sse_completions("COPILOT completions hello"),
            ),
            ("POST", "/messages") => (
                200,
                "text/event-stream",
                sse_anthropic("COPILOT anthropic hello"),
            ),
            _ => (404, json, json!({"error": "not found"}).to_string()),
        }
    }));

    // RED
    {
        let agent = tmp.join("agent-red");
        std::fs::create_dir_all(&agent).unwrap();
        let out = run_pi(
            &[
                "--provider",
                "github-copilot",
                "--model",
                "gpt-5.6-sol",
                "--prompt",
                "hi",
            ],
            &agent,
            &[],
        );
        assert_ne!(out.status.code(), Some(0), "RED must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not supported in v0"),
            "RED: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    install_bundled("github-copilot");

    // LOGIN: device flow → exchange → models (no policy enables: the mock
    // catalog is all enabled).
    let agent_login = tmp.join("agent-login");
    std::fs::create_dir_all(&agent_login).unwrap();
    let out = Command::new(pi_bin())
        .args(["auth", "login", "github-copilot", "--no-browser"])
        .env("PI_CODING_AGENT_DIR", &agent_login)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("GITHUB_COPILOT_DEVICE_HOST", &mock.base)
        .env("GITHUB_COPILOT_BASE_URL", &mock.base)
        .output()
        .expect("running pi-rust auth login");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "login\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("Logged in to github-copilot"),
        "login\n{stdout}"
    );
    assert!(stdout.contains("CP-MOCK"), "user code\n{stdout}");
    let cred = &read_auth(&agent_login)["github-copilot"];
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("cp-mock-access-1"));
    assert_eq!(
        cred["refresh"],
        json!("gh-mock-token"),
        "the GitHub token is the refresh material"
    );
    assert!(cred["expires"].as_i64().unwrap_or(0) > now_ms());
    // The account model catalog lands in the credential (upstream shape).
    let ids = cred["availableModelIds"]
        .as_array()
        .expect("availableModelIds");
    assert!(
        ids.iter().any(|v| v == "gpt-5.6-sol"),
        "the fetched catalog must be stored: {cred}"
    );
    // The exchange used the GitHub token (Bearer) and the models fetch
    // carried the API version header.
    let exchange = mock.to("/copilot_internal/v2/token");
    assert_eq!(
        exchange[0].header("authorization").as_deref(),
        Some("Bearer gh-mock-token"),
        "the exchange must use the GitHub token"
    );
    let models = mock.to("/models");
    assert!(!models.is_empty());
    assert_eq!(
        models[0].header("x-github-api-version").as_deref(),
        Some("2026-06-01"),
        "the models fetch must carry the copilot API version"
    );

    // STREAM + REFRESH (openai-responses wire): an expired credential →
    // re-exchange (cp-mock-access-2) + re-fetch models + rotated write-back.
    let agent_run = tmp.join("agent-run");
    std::fs::create_dir_all(&agent_run).unwrap();
    std::fs::write(
        agent_run.join("auth.json"),
        r#"{"github-copilot":{"type":"oauth","access":"cp-old","refresh":"gh-mock-token","expires":1,"availableModelIds":["gpt-5.6-sol"]}}"#,
    )
    .unwrap();
    let session = tmp.join("session-cp.jsonl");
    let out = Command::new(pi_bin())
        .arg("--provider")
        .arg("github-copilot")
        .arg("--model")
        .arg("gpt-5.6-sol")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_run)
        .arg("--session-file")
        .arg(&session)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
        .env("GITHUB_COPILOT_BASE_URL", &mock.base)
        .output()
        .expect("running pi-rust github-copilot");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "run\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "degraded\n{stderr}"
    );
    assert!(
        session_text(&session).contains("COPILOT responses hello"),
        "the plugin must have served the stream\n{}",
        session_text(&session)
    );
    let cred = &read_auth(&agent_run)["github-copilot"];
    assert_eq!(
        cred["access"],
        json!("cp-mock-access-2"),
        "the re-exchanged token must be written back"
    );
    let stream_req = mock.to("/responses");
    assert_eq!(
        stream_req[0].header("authorization").as_deref(),
        Some("Bearer cp-mock-access-2"),
        "the stream must use the re-exchanged token"
    );

    // The other two wire APIs (fresh, non-expired credential → no refresh):
    // anthropic-messages + openai-completions.
    let agent_wire = tmp.join("agent-wire");
    std::fs::create_dir_all(&agent_wire).unwrap();
    let fresh_expires = now_ms() + 3_600_000;
    std::fs::write(
        agent_wire.join("auth.json"),
        serde_json::to_string(&json!({
            "github-copilot": {
                "type": "oauth",
                "access": "cp-mock-access-1",
                "refresh": "gh-mock-token",
                "expires": fresh_expires,
                "availableModelIds": ["claude-sonnet-5", "gemini-3.8-flash"],
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let exchanges_before = mock.to("/copilot_internal/v2/token").len();
    for (model, text) in [
        ("claude-sonnet-5", "COPILOT anthropic hello"),
        ("gemini-3.8-flash", "COPILOT completions hello"),
    ] {
        let session = tmp.join(format!("session-{model}.jsonl"));
        let out = Command::new(pi_bin())
            .arg("--provider")
            .arg("github-copilot")
            .arg("--model")
            .arg(model)
            .arg("--prompt")
            .arg("hi")
            .arg("--agent-dir")
            .arg(&agent_wire)
            .arg("--session-file")
            .arg(&session)
            .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s2-node")
            .env("GITHUB_COPILOT_BASE_URL", &mock.base)
            .output()
            .expect("running pi-rust wire");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{model}\n{stdout}\n{stderr}");
        assert!(
            session_text(&session).contains(text),
            "{model}: the plugin must have served the stream\n{}",
            session_text(&session)
        );
    }
    // No refresh on the fresh credential (no extra exchange — the count is
    // exactly the login's exchange + the refresh run's re-exchange).
    assert_eq!(
        mock.to("/copilot_internal/v2/token").len(),
        exchanges_before,
        "a fresh credential must not refresh"
    );
    // The anthropic wire carried the copilot dynamic headers.
    let anthro = mock.to("/messages");
    assert!(!anthro.is_empty());
    assert_eq!(
        anthro[0].header("openai-intent").as_deref(),
        Some("conversation-edits"),
        "the copilot dynamic headers must ride the anthropic wire"
    );
    for secret in ["cp-mock-access-1", "cp-mock-access-2", "gh-mock-token"] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "token material {secret} leaked into the process output"
        );
    }
    remove_bundled("github-copilot");
    let _ = std::fs::remove_dir_all(&tmp);
}
