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
sys.stderr.write("prompt-seen=%s\n" % ok)
os.write(fd, KEY.encode() + b"\r")
ok2 = read_until(b"Saved API key for Anthropic")
ok3 = read_until(b"Credentials saved to")
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
sys.exit(0 if (ok and ok2 and ok3 and code == 0
               and anthropic.get("type") == "api_key"
               and anthropic.get("key") == KEY
               and KEY.encode() not in buf) else 11)
"#;

#[test]
fn auth_login_api_key_stores_upstream_credential() {
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
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(res.status.success(), "auth login PTY harness failed: {out}");
}
