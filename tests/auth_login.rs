//! WP-1007 acceptance: `pi-rust auth login` runs the pinned upstream's
//! terminal login flow unmodified (dedicated sidecar/auth-login.mjs); the
//! core owns the TTY and relays prompts over NDJSON.
//!
//! The --api-key path is exercised for real (no network): it prompts for
//! the Anthropic API key and stores the upstream-shaped api_key credential
//! in the shared auth.json. The account (oauth) flows are upstream code
//! unmodified; their relay shares the same prompt machinery this test
//! drives.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bin() -> PathBuf {
    repo_root().join("target/debug/pi-rust")
}

fn args_error(args: &[&str]) -> (i32, String) {
    let out = Command::new(bin())
        .args(args)
        .env(
            "PI_CODING_AGENT_DIR",
            "/tmp/should-not-be-created-auth-test",
        )
        .output()
        .expect("running pi-rust auth");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

#[test]
fn auth_login_rejects_bad_arg_shapes() {
    // No subcommand / no provider / both flags: all exit 1 with usage or a
    // precise error, and never spawn the helper (no TTY in the test).
    let (code, out) = args_error(&["auth"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("usage: pi-rust auth login"), "{out}");

    let (code, out) = args_error(&["auth", "login"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("usage: pi-rust auth login"), "{out}");

    let (code, out) = args_error(&["auth", "login", "xai", "--api-key", "--no-browser"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("--no-browser only applies to account sign-in"),
        "{out}"
    );

    // Unknown provider: the helper runs the upstream flow, which reports the
    // provider list — but without a TTY the core stops first.
    let (code, out) = args_error(&["auth", "login", "no-such-provider"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("requires an interactive terminal"), "{out}");
}

const PYTHON_LOGIN: &str = r#"
#!/usr/bin/env python3
# WP-1007 acceptance: auth login anthropic --api-key through a real PTY.
import json, os, pty, re, select, sys, time

BIN = os.path.join(sys.argv[1], "target", "debug", "pi-rust")
ROOT = sys.argv[1]
AGENT_DIR = sys.argv[2]
KEY = "sk-ant-e2e-test-key"

pid, fd = pty.fork()
if pid == 0:
    os.environ["PI_CODING_AGENT_DIR"] = AGENT_DIR
    os.environ["PI_RUST_SIDECAR"] = os.path.join(ROOT, "sidecar", "index.mjs")
    os.chdir(ROOT)
    os.execvp(BIN, [BIN, "auth", "login", "anthropic", "--api-key"])

buf = b""
t0 = time.time()
times = {"t0": 0.0}
def read_until(needle: bytes, timeout=45.0) -> bool:
    global buf
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return needle in buf
            if not data:
                return needle in buf
            buf += data
        if needle in buf:
            return True
    return needle in buf

ok = read_until(b"Enter Anthropic API key")
times["prompt"] = time.time() - t0
sys.stderr.write("prompt-seen=%s\n" % ok)
os.write(fd, KEY.encode() + b"\r")
times["wrote_key"] = time.time() - t0
# 120s: a starved CI runner can delay the sidecar's event loop well past the
# local 45s; correctness is still gated by the saved/shape/key checks below.
ok2 = read_until(b"Saved API key for Anthropic", timeout=120.0)
times["saved"] = time.time() - t0
ok3 = read_until(b"Credentials saved to", timeout=120.0)
times["saved_to"] = time.time() - t0
sys.stderr.write("saved=%s saved-to=%s\n" % (ok2, ok3))

time.sleep(1.0)  # let the catalog-refresh note and the exit land
try:
    os.close(fd)
except OSError:
    pass
_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)

auth_path = os.path.join(AGENT_DIR, "auth.json")
stored = None
if os.path.exists(auth_path):
    with open(auth_path) as f:
        stored = json.load(f)
anthropic = (stored or {}).get("anthropic") or {}
sys.stderr.write("credential-shape=%s\n" % (
    anthropic.get("type") == "api_key" and anthropic.get("key") == KEY
))
# Redaction: the key is typed without echo and must never appear in output.
sys.stderr.write("key-in-output=%s\n" % (KEY.encode() in buf))
sys.stderr.write("===EXIT %d\n" % code)
sys.stderr.write("TIMINGS %s\n" % {k: round(v, 1) for k, v in times.items()})
if not (ok and ok2 and ok3):
    # Diagnostic transcript (key redacted): which prompt phase was the PTY
    # actually in when the key was written?
    transcript = buf.decode("utf-8", "replace").replace(KEY, "[REDACTED-KEY]")
    sys.stderr.write("TRANSCRIPT:\n" + transcript[-4000:] + "\nEND-TRANSCRIPT\n")
sys.exit(0 if (ok and ok2 and ok3 and code == 0
               and anthropic.get("type") == "api_key"
               and anthropic.get("key") == KEY
               and KEY.encode() not in buf) else 11)
"#;

#[test]
fn auth_login_api_key_stores_upstream_credential() {
    // WP-1008: the PTY harness needs the POSIX pty module; Windows runs the
    // headless suite only (the `--api-key` path itself is plain stdio and
    // stays covered by the arg-shape test below).
    if cfg!(windows) {
        eprintln!("skipping: PTY harness unavailable on Windows (WP-1008)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("pi-rust-auth-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();

    let py = tmp.join("login_harness.py");
    std::fs::write(&py, PYTHON_LOGIN).unwrap();

    let child = Command::new("python3")
        .arg(&py)
        .arg(repo_root())
        .arg(&agent_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning the PTY harness");
    // The login flow is user-paced; the scripted harness needs no pacing.
    let res = child.wait_with_output().expect("harness exited");
    let mut out = String::from_utf8_lossy(&res.stdout).to_string();
    out.push_str(&String::from_utf8_lossy(&res.stderr));
    if !res.status.success() {
        // Keep the temp dir for post-mortem (agent dir + auth.json state).
        eprintln!("preserving {tmp:?}");
    } else {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    assert!(res.status.success(), "auth login PTY harness failed: {out}");
}

const PYTHON_KIMI_LOGIN: &str = r#"
#!/usr/bin/env python3
# E3S4/WP-1404 acceptance: auth login kimi-coding through a real PTY — the
# CORE performs the device flow itself (mock token endpoint). The child's
# PI_RUST_NODE points at a nonexistent binary: if the core still needed the
# node helper for this login, it would fail.
import json, os, pty, select, sys, time

BIN = os.path.join(sys.argv[1], "target", "debug", "pi-rust")
ROOT = sys.argv[1]
AGENT_DIR = sys.argv[2]
OAUTH_HOST = sys.argv[3]

pid, fd = pty.fork()
if pid == 0:
    os.environ["PI_CODING_AGENT_DIR"] = AGENT_DIR
    os.environ["PI_RUST_NODE"] = "/nonexistent/pi-rust-wp1404-node"
    os.environ["KIMI_CODE_OAUTH_HOST"] = OAUTH_HOST
    os.chdir(ROOT)
    os.execvp(BIN, [BIN, "auth", "login", "kimi-coding", "--no-browser"])

buf = b""
def read_until(needle: bytes, timeout=60.0) -> bool:
    global buf
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return needle in buf
            if not data:
                return needle in buf
            buf += data
        if needle in buf:
            return True
    return needle in buf

# Device flow UX (upstream wording): the verification URL + user code, then
# the wait line; the mock completes the first poll, so the success line
# lands seconds later.
ok1 = read_until(b"Waiting for authentication...")
ok2 = read_until(b"Logged in to Kimi For Coding")
ok3 = read_until(b"Credentials saved to")
time.sleep(1.0)
try:
    os.close(fd)
except OSError:
    pass
_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)

auth_path = os.path.join(AGENT_DIR, "auth.json")
stored = None
if os.path.exists(auth_path):
    with open(auth_path) as f:
        stored = json.load(f)
kimi = (stored or {}).get("kimi-coding") or {}
sys.stderr.write("wait-seen=%s logged=%s saved-to=%s\n" % (ok1, ok2, ok3))
sys.stderr.write("credential-shape=%s\n" % (
    kimi.get("type") == "oauth"
    and kimi.get("access") == "new-access"
    and kimi.get("refresh") == "new-refresh"))
# The user code is displayed by design (not a secret); the tokens are not.
sys.stderr.write("token-in-output=%s\n" % (b"new-access" in buf or b"new-refresh" in buf))
sys.stderr.write("===EXIT %d\n" % code)
if not (ok1 and ok2 and ok3):
    sys.stderr.write("TRANSCRIPT:\n" + buf.decode("utf-8", "replace")[-4000:] + "\nEND-TRANSCRIPT\n")
sys.exit(0 if (ok1 and ok2 and ok3 and code == 0
               and kimi.get("type") == "oauth"
               and kimi.get("access") == "new-access"
               and kimi.get("refresh") == "new-refresh"
               and b"new-access" not in buf and b"new-refresh" not in buf) else 11)
"#;

#[test]
fn auth_login_kimi_device_flow_core_native_node_free() {
    // E3S4: the kimi-coding account login is core-native — it must work with
    // NO node (the child's PI_RUST_NODE does not exist). The PTY harness
    // needs the POSIX pty module; Windows runs the headless suite only
    // (the arg-shape test above still covers it).
    if cfg!(windows) {
        eprintln!("skipping: PTY harness unavailable on Windows (WP-1008)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("pi-rust-kimi-login-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();

    // Mock oauth endpoint (device_authorization + token), all local.
    let log_file = tmp.join("oauth-log.jsonl");
    let mut child = Command::new("node")
        .arg(repo_root().join("tests/fixtures/kimi-oauth-mock.mjs"))
        .arg(&log_file)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawning mock kimi oauth endpoint");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("mock stdout"));
    let mut port_line = String::new();
    use std::io::BufRead;
    stdout.read_line(&mut port_line).expect("mock port line");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("mock port");
    let oauth_host = format!("http://127.0.0.1:{port}");

    let py = tmp.join("kimi_login_harness.py");
    std::fs::write(&py, PYTHON_KIMI_LOGIN).unwrap();
    let res = Command::new("python3")
        .arg(&py)
        .arg(repo_root())
        .arg(&agent_dir)
        .arg(&oauth_host)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawning the PTY harness")
        .wait_with_output()
        .expect("harness exited");
    let _ = child.kill();
    let _ = child.wait();
    let mut out = String::from_utf8_lossy(&res.stdout).to_string();
    out.push_str(&String::from_utf8_lossy(&res.stderr));
    let log = std::fs::read_to_string(&log_file).unwrap_or_default();
    if !res.status.success() {
        eprintln!("preserving {tmp:?}");
    } else {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    assert!(res.status.success(), "kimi login PTY harness failed: {out}");

    // The login flow hit the mock: device authorization + one device-code
    // token poll (upstream grant type).
    assert!(log.contains("device_authorization"), "{log}");
    assert!(
        log.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
            || log.contains("grant_type=urn:ietf:params:oauth:grant-type:device_code"),
        "{log}"
    );
}
