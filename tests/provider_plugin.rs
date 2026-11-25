//! E4S1/WP-1501 hard acceptance: provider plugin ABI + directory loader.
//!
//! The sample plugin (`plugin-samples/openai-compat`, built on demand) is
//! the fixture. With the sidecar DEGRADED (spawn failure — the Node binary
//! does not exist) the run must be Node-free:
//! 1. LOGIN: `pi-rust auth login sample-compat --no-browser` — the core
//!    drives the plugin's mock loopback handshake, satisfies nothing itself,
//!    and stores the returned credential in auth.json (the plugin never
//!    touches the store);
//! 2. STREAM: `--provider sample-compat --model bundled-model` — selectable
//!    like a built-in, streamed through the plugin against a mock OpenAI SSE
//!    endpoint; the core-resolved credential (from auth.json) reaches the
//!    wire as the Authorization header (the ABI's credential path);
//! 3. FAILURE: a garbage `.so` and a wrong-`abi_version` cdylib in the user
//!    dir → typed log lines, the run continues with the remaining plugins;
//! 4. OVERRIDE: a user-dir plugin shadowing the bundled name wins (logged).

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pi-rust"))
}

fn plugin_ext() -> &'static str {
    if cfg!(windows) {
        "dll"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    }
}

/// Builds the sample plugin fixture (`cargo build -p pi-sample-compat`,
/// shared workspace target dir — the normal `cargo build`/`cargo test`
/// does NOT build it; the CI `cargo test` covers it through this test).
fn sample_so() -> PathBuf {
    // The sample is a standalone crate (its own workspace — the normal
    // `cargo build`/`cargo test` never compile it); the e2e builds it on
    // demand, so the CI `cargo test` on all three OSes still covers it.
    let sample_dir = repo_root().join("plugin-samples/openai-compat");
    let target = sample_dir.join("target/debug");
    // The cdylib file name uses the lib target name (dashes → underscores).
    let artifact = if cfg!(windows) {
        target.join("pi_sample_compat.dll")
    } else if cfg!(target_os = "macos") {
        target.join("libpi_sample_compat.dylib")
    } else {
        target.join("libpi_sample_compat.so")
    };
    let st = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(sample_dir.join("Cargo.toml"))
        .current_dir(&sample_dir)
        .output()
        .expect("running cargo build for the sample plugin");
    assert!(
        st.status.success(),
        "building the sample plugin failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );
    assert!(
        artifact.exists(),
        "sample artifact missing: {}",
        artifact.display()
    );
    artifact
}

/// Mock OpenAI SSE endpoint; records each request's `authorization` header
/// (the credential path assertion).
fn sse_auth_mock(text: &str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let recorded: Arc<Mutex<Vec<String>>> = Arc::default();
    let rec2 = recorded.clone();
    let text = text.to_string();
    std::thread::spawn(move || loop {
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            match s.read(&mut tmp) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf).to_string();
        let head_lc = head.to_lowercase();
        let auth = head_lc
            .lines()
            .find_map(|l| l.strip_prefix("authorization:"))
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        rec2.lock().unwrap().push(auth);
        let c1 = serde_json::json!({"choices":[{"delta":{"role":"assistant","content":text}}]})
            .to_string();
        let sse = format!("data: {c1}\ndata: [DONE]\n");
        let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
        for line in sse.split_inclusive('\n') {
            if line.trim().is_empty() {
                continue;
            }
            let b = line.as_bytes();
            body.push_str(&format!("{:x}\r\n", b.len()));
            body.push_str(line);
            body.push_str("\r\n");
        }
        body.push_str("0\r\n\r\n");
        let _ = s.write_all(body.as_bytes());
    });
    (base, recorded)
}

fn event_log(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
        .collect()
}

fn tmp_root(tag: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("pi-rust-e4s1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    tmp
}

/// Atomic copy (temp name + rename) so parallel pi-rust runs never see a
/// half-written plugin file.
fn copy_atomic(src: &Path, dst: &Path) {
    let staged = dst.with_file_name(format!(
        ".{}.staged",
        dst.file_name().unwrap().to_string_lossy()
    ));
    std::fs::copy(src, &staged).unwrap();
    std::fs::rename(&staged, dst).unwrap();
}

/// Satisfies the mock loopback: a plain GET on the URL the login printed.
fn http_get(url: &str) {
    let hp = url
        .strip_prefix("http://")
        .expect("loopback http url")
        .split_once('/')
        .map(|(h, _)| h)
        .unwrap_or_default();
    let mut s = TcpStream::connect(hp).expect("connecting to the plugin loopback");
    let req = format!("GET / HTTP/1.1\r\nhost: {hp}\r\nconnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    assert!(
        resp.starts_with(b"HTTP/1.1 200"),
        "loopback callback not accepted: {resp:?}"
    );
}

/// The wrong-`abi_version` stub (a minimal cdylib compiled with rustc —
/// dependency-free, so no cargo build is needed).
const WRONG_VERSION_STUB: &str = r#"
#[repr(C)]
struct Vt {
    abi_version: extern "C" fn() -> u32,
    name: extern "C" fn() -> *const u8,
    list_models: extern "C" fn() -> *const u8,
    stream_open: extern "C" fn(*const u8) -> *const u8,
    stream_next: extern "C" fn(*const u8) -> *const u8,
    stream_close: extern "C" fn(*const u8),
    auth_start: extern "C" fn(*const u8) -> *const u8,
    auth_status: extern "C" fn(*const u8) -> *const u8,
    dispose: extern "C" fn(),
}
extern "C" fn abi_version() -> u32 { 999 }
static NAME: &[u8] = b"wrongver\0";
extern "C" fn name() -> *const u8 { NAME.as_ptr() }
extern "C" fn list_models() -> *const u8 { b"[]\0".as_ptr() }
extern "C" fn stream_open(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn stream_next(_h: *const u8) -> *const u8 { std::ptr::null() }
extern "C" fn stream_close(_h: *const u8) {}
extern "C" fn auth_start(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn auth_status(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn dispose() {}
static VT: Vt = Vt { abi_version, name, list_models, stream_open, stream_next, stream_close, auth_start, auth_status, dispose };
#[no_mangle] pub extern "C" fn pi_provider_plugin_init() -> *const Vt { &VT }
"#;

fn compile_wrong_version(dir: &Path) -> PathBuf {
    let out = dir.join(format!("pi-wrong.{ext}", ext = plugin_ext()));
    let src = dir.join("pi-wrong.rs");
    std::fs::write(&src, WRONG_VERSION_STUB).unwrap();
    let st = Command::new("rustc")
        .args(["--edition", "2021", "--crate-type", "cdylib", "-O", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("running rustc");
    assert!(
        st.status.success(),
        "rustc failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );
    out
}

#[test]
fn e2e_provider_plugin() {
    let tmp = tmp_root("main");
    let agent = tmp.join("agent");
    std::fs::create_dir_all(&agent).unwrap();

    // The sample plugin + the bundled dir next to the test binary
    // (<exe dir>/providers/ — the E4S1 bundled layout).
    let sample = sample_so();
    let exe_dir = bin().parent().unwrap().to_path_buf();
    let bundled_dir = exe_dir.join("providers");
    std::fs::create_dir_all(&bundled_dir).unwrap();
    let bundled_copy = bundled_dir.join(format!("libpi_sample_compat.{ext}", ext = plugin_ext()));
    copy_atomic(&sample, &bundled_copy);

    // (1) LOGIN — the core-driven handshake (headless, --no-browser). The
    // e2e satisfies the mock loopback; the core stores the credential.
    let mut child = Command::new(bin())
        .args(["auth", "login", "sample-compat", "--no-browser"])
        .env("PI_CODING_AGENT_DIR", &agent)
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s1-node")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("running auth login");
    let stdout = child.stdout.take().unwrap();
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let lines2 = lines.clone();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(l) = line else { continue };
            lines2.lock().unwrap().push(l);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let url = loop {
        let found = lines
            .lock()
            .unwrap()
            .iter()
            .find(|l| l.starts_with("Sign in: "))
            .map(|l| l["Sign in: ".len()..].trim().to_string());
        if let Some(u) = found {
            break u;
        }
        assert!(
            Instant::now() < deadline,
            "no sign-in URL:\n{}",
            lines.lock().unwrap().join("\n")
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    http_get(&url); // the e2e satisfies the mock loopback
    let wait_deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                assert!(
                    Instant::now() < wait_deadline,
                    "auth login timed out (lines: {})",
                    lines.lock().unwrap().join("\n")
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("waiting for auth login: {e}"),
        }
    };
    let exit = status.code().unwrap_or(-1);
    let out = lines.lock().unwrap().join("\n");
    assert_eq!(exit, 0, "login failed: url={url}\n{out}");
    reader.join().unwrap();
    // The core (not the plugin) wrote auth.json.
    let auth_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(agent.join("auth.json")).unwrap()).unwrap();
    let cred = &auth_json["sample-compat"];
    assert_eq!(cred["type"], "api_key");
    assert!(
        cred["key"]
            .as_str()
            .is_some_and(|k| k.starts_with("mock-key-")),
        "credential shape: {cred:?}"
    );

    // (2) STREAM — sidecar degraded (spawn failure), the credential from
    // auth.json travels through the ABI to the wire.
    let (base, auth_headers) = sse_auth_mock("Hello from sample plugin");
    let run = |extra_env: &[(&str, &str)], args: &[&str]| -> (i32, String, String) {
        let session_file = tmp.join(format!("session-{}.jsonl", uuid::Uuid::new_v4().simple()));
        let mut cmd = Command::new(bin());
        for a in args {
            cmd.arg(a);
        }
        cmd.env("PI_RUST_NODE", "/nonexistent/pi-rust-e4s1-node")
            .env("PI_SAMPLE_BASE_URL", &base)
            .arg("--agent-dir")
            .arg(&agent)
            .arg("--session-file")
            .arg(&session_file);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("running pi-rust");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };
    let (code, stdout, stderr) = run(
        &[],
        &[
            "--provider",
            "sample-compat",
            "--model",
            "bundled-model",
            "--prompt",
            "hi",
        ],
    );
    assert_eq!(code, 0, "stream run failed:\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path (Node-free)\n{stderr}"
    );
    assert!(
        stderr.contains("provider plugin loaded: sample-compat"),
        "the bundled plugin must load at startup\n{stderr}"
    );
    let lines = event_log(&stdout);
    assert_eq!(lines.last().unwrap()["kind"], "transcript");
    let dump = lines.last().unwrap()["session"].to_string();
    assert!(
        dump.contains("Hello from sample plugin"),
        "the plugin must have served the stream\n{dump}"
    );
    assert!(
        auth_headers
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.starts_with("bearer mock-key-")),
        "the core-resolved credential must reach the wire: {:?}",
        auth_headers.lock().unwrap()
    );

    // (3) FAILURE — garbage + wrong abi_version in the USER dir: typed log
    // lines, the run continues (the bundled plugin still serves).
    let user_prov = agent.join("providers");
    std::fs::create_dir_all(&user_prov).unwrap();
    let garbage: &[u8] = if plugin_ext() == "dll" {
        b"MZ"
    } else {
        b"\x7fELF\x02\x01\x01 truncated"
    };
    std::fs::write(
        user_prov.join(format!("pi-garbage.{ext}", ext = plugin_ext())),
        garbage,
    )
    .unwrap();
    let wrong_dir = tmp.join("wrong");
    std::fs::create_dir_all(&wrong_dir).unwrap();
    let wrong = compile_wrong_version(&wrong_dir);
    copy_atomic(
        &wrong,
        &user_prov.join(format!("pi-wrong.{ext}", ext = plugin_ext())),
    );
    let (code, stdout, stderr) = run(
        &[],
        &[
            "--provider",
            "sample-compat",
            "--model",
            "bundled-model",
            "--prompt",
            "hi",
        ],
    );
    assert_eq!(
        code, 0,
        "the run must continue past failing plugins:\n{stderr}"
    );
    let lines = event_log(&stdout);
    assert!(lines.last().unwrap()["session"]
        .to_string()
        .contains("Hello from sample plugin"));
    assert!(
        stderr.contains("pi-garbage") && stderr.contains("failed to load"),
        "garbage plugin: typed log line\n{stderr}"
    );
    assert!(
        stderr.contains("abi_version mismatch"),
        "wrong version: typed log line\n{stderr}"
    );

    // (4) OVERRIDE — a user-dir plugin with the same name shadows the
    // bundled one (logged); its model list wins wholesale.
    copy_atomic(
        &sample,
        &user_prov.join(format!("libpi-user.{ext}", ext = plugin_ext())),
    );
    let (code, _stdout, stderr) = run(
        &[("PI_SAMPLE_VARIANT", "user")],
        &[
            "--provider",
            "sample-compat",
            "--model",
            "user-model",
            "--prompt",
            "hi",
        ],
    );
    assert_eq!(code, 0, "user-dir model must resolve:\n{stderr}");
    assert!(
        stderr.contains("override") && stderr.contains("shadows"),
        "the override must be logged\n{stderr}"
    );
    // The bundled variant's model is gone (the whole plugin was replaced).
    let (code, _stdout, stderr) = run(
        &[("PI_SAMPLE_VARIANT", "user")],
        &[
            "--provider",
            "sample-compat",
            "--model",
            "bundled-model",
            "--prompt",
            "hi",
        ],
    );
    assert_ne!(code, 0, "shadowed bundled model must not resolve");
    assert!(
        stderr.contains("is not provided by provider plugin"),
        "typed error for the shadowed model\n{stderr}"
    );

    // Cleanup: the bundled dir copy is ours (only the e2e writes there).
    let _ = std::fs::remove_file(&bundled_copy);
    let _ = std::fs::remove_dir_all(&tmp);
}
