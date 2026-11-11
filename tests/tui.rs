//! WP-601 acceptance: integration smoke through the real binary under a PTY —
//! enter the TUI, send a message, the scripted fake model reply is rendered,
//! ctrl+d (upstream app.exit) quits with exit 0.
//!
//! The python harness decodes the escape stream into a character grid
//! (ratatui's diff output positions text with cursor jumps, so raw byte
//! searches do not work on the stream).
use std::process::Command;

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const PYTHON: &str = r#"
import os, pty, select, sys, time, fcntl, termios, struct, re

BIN = os.path.join(sys.argv[1], "target", "debug", "pi-rust")
WORKDIR = sys.argv[2]
AGENT_DIR = sys.argv[3]

pid, fd = pty.fork()
if pid == 0:
    os.environ["PI_RUST_SIDECAR"] = os.path.join(sys.argv[1], "sidecar", "index.mjs")
    os.chdir(WORKDIR)
    os.execvp(BIN, [BIN, "--provider", "fake", "--agent-dir", AGENT_DIR])

# pty.fork() defaults to a 0x0 window; give the TUI an 80x24 screen.
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

ROWS, COLS = 24, 80
grid = [[" "] * COLS for _ in range(ROWS)]
cur = [0, 0]

def feed(data: bytes):
    i, n = 0, len(data)
    while i < n:
        b = data[i:i+1]
        if b == b"\x1b":
            m = re.match(rb"\x1b\[(\d+);(\d+)H", data[i:])
            if m:
                cur[0] = min(int(m.group(1)) - 1, ROWS - 1)
                cur[1] = min(int(m.group(2)) - 1, COLS - 1)
                i += len(m.group(0))
                continue
            m = re.match(rb"\x1b\[[0-9;?]*[A-Za-z]", data[i:])
            if m:
                i += len(m.group(0))
                continue
            m = re.match(rb"\x1b.", data[i:])
            if m:
                i += 2
                continue
            i += 1
            continue
        if b == b"\r":
            cur[1] = 0
            i += 1
            continue
        if b == b"\n":
            cur[0] = min(cur[0] + 1, ROWS - 1)
            i += 1
            continue
        if 32 <= data[i] < 127:
            if cur[0] < ROWS and cur[1] < COLS:
                grid[cur[0]][cur[1]] = chr(data[i])
            cur[1] = min(cur[1] + 1, COLS - 1)
        i += 1

def screen_text():
    return "\n".join("".join(r).rstrip() for r in grid)

def wait_for(needle: bytes, timeout=15.0) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return needle in screen_text().encode("latin-1")
            if not data:
                return needle in screen_text().encode("latin-1")
            feed(data)
            if needle in screen_text().encode("latin-1"):
                return True
    return needle in screen_text().encode("latin-1")

ok = wait_for(b"fake/fake")                      # TUI ready (status bar)
os.write(fd, b"hello\r")                          # raw-mode Enter is 0x0D
ok = wait_for(b"echo: hello") and ok             # fake model reply rendered
os.write(fd, b"\x04")                             # ctrl+d: app.exit (empty editor)

time.sleep(0.5)
try:
    os.close(fd)
except OSError:
    pass
_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)
sys.stderr.write(screen_text())
sys.stderr.write("\n===EXIT %d OK=%d\n" % (code, int(ok)))
sys.exit(0 if (code == 0 and ok) else 11)
"#;

const PYTHON_DIALOG: &str = r#"
#!/usr/bin/env python3
# WP-604 acceptance: extension dialogs rendered in the TUI, answered by
# the user, answers received by the extension over the unchanged bridge
# protocol. The fixture command fails (extension_error) unless the answers
# are exactly confirm=yes, select=beta, input=amy.
import os, pty, select, sys, time, fcntl, termios, struct, re

BIN = os.path.join(sys.argv[1], "target", "debug", "pi-rust")
WORKDIR = sys.argv[2]
AGENT_DIR = sys.argv[3]
EXT_DIR = sys.argv[4]

pid, fd = pty.fork()
if pid == 0:
    os.environ["PI_RUST_SIDECAR"] = os.path.join(sys.argv[1], "sidecar", "index.mjs")
    os.chdir(WORKDIR)
    os.execvp(BIN, [BIN, "--provider", "fake", "--agent-dir", AGENT_DIR,
                    "--extension-dir", EXT_DIR, "--command", "dialog-check"])

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))

ROWS, COLS = 24, 80
grid = [[" "] * COLS for _ in range(ROWS)]
cur = [0, 0]

def feed(data: bytes):
    i, n = 0, len(data)
    while i < n:
        b = data[i:i+1]
        if b == b"\x1b":
            m = re.match(rb"\x1b\[(\d+);(\d+)H", data[i:])
            if m:
                cur[0] = min(int(m.group(1)) - 1, ROWS - 1)
                cur[1] = min(int(m.group(2)) - 1, COLS - 1)
                i += len(m.group(0))
                continue
            m = re.match(rb"\x1b\[[0-9;?]*[A-Za-z]", data[i:])
            if m:
                i += len(m.group(0))
                continue
            m = re.match(rb"\x1b.", data[i:])
            if m:
                i += 2
                continue
            i += 1
            continue
        if b == b"\r":
            cur[1] = 0
            i += 1
            continue
        if b == b"\n":
            cur[0] = min(cur[0] + 1, ROWS - 1)
            i += 1
            continue
        if 32 <= data[i] < 127:
            if cur[0] < ROWS and cur[1] < COLS:
                grid[cur[0]][cur[1]] = chr(data[i])
            cur[1] = min(cur[1] + 1, COLS - 1)
        i += 1

def screen_text():
    return "\n".join("".join(r).rstrip() for r in grid)

def wait_for(needle: bytes, timeout=15.0) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return needle in screen_text().encode("latin-1")
            if not data:
                return needle in screen_text().encode("latin-1")
            feed(data)
            if needle in screen_text().encode("latin-1"):
                return True
    return needle in screen_text().encode("latin-1")

steps = []
steps.append(wait_for(b"fake/fake"))                    # TUI ready
steps.append(wait_for(b"Proceed?"))                      # confirm dialog rendered
os.write(fd, b"y")                                       # yes
steps.append(wait_for(b"beta"))                          # select dialog rendered
os.write(fd, b"\x1b[B\r")                                # down (beta) + enter
steps.append(wait_for(b"type here"))                     # input dialog rendered
os.write(fd, b"amy\r")                                   # input + submit
steps.append(wait_for(b"dialog done"))                     # notify only fires after all answers matched
steps.append(not wait_for(b"dialog answers wrong", timeout=2.0))  # no extension_error
os.write(fd, b"\x04")                                    # ctrl+d exit

time.sleep(0.5)
try:
    os.close(fd)
except OSError:
    pass
_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)
sys.stderr.write(screen_text())
sys.stderr.write("\n===EXIT %d STEPS %s\n" % (code, "".join(map(str, steps))))
sys.exit(0 if (code == 0 and all(steps)) else 11)
"#;

#[test]
fn tui_extension_dialog_roundtrip() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-tui-dlg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let py = tmp.join("dialog_harness.py");
    std::fs::write(&py, PYTHON_DIALOG).unwrap();

    let out = Command::new("python3")
        .args([
            py.display().to_string().as_str(),
            repo().display().to_string().as_str(),
            tmp.display().to_string().as_str(),
            tmp.join("agent-dir").display().to_string().as_str(),
            repo().join("tests/fixtures/dialogs").display().to_string().as_str(),
        ])
        .output()
        .expect("running pty harness");
    let screen = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(code, 0, "pty dialog harness failed (child exit/dialog not answered). Screen:\n{screen}");
}

#[test]
fn tui_interactive_smoke() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-tui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let py = tmp.join("tui_harness.py");
    std::fs::write(&py, PYTHON).unwrap();

    let out = Command::new("python3")
        .args([
            py.display().to_string().as_str(),
            repo().display().to_string().as_str(),
            tmp.display().to_string().as_str(),
            tmp.join("agent-dir").display().to_string().as_str(),
        ])
        .output()
        .expect("running pty harness");
    let screen = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
    assert_eq!(code, 0, "pty harness failed (child exit/missing content). Screen:\n{screen}");
    assert!(screen.contains("OK=1"), "harness did not confirm TUI interaction:\n{screen}");
}
