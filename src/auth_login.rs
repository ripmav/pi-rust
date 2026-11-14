//! `pi-rust auth login <provider> [--api-key] [--no-browser]` (WP-1007).
//!
//! The pinned upstream's terminal login flow runs UNMODIFIED in
//! `sidecar/auth-login.mjs` (`ModelRuntime.create` + `runAuthLogin`); the
//! core owns the user's TTY and relays prompts/notifications over NDJSON.
//! Key material only arrives as prompt answers and is never logged (the
//! WP-904 redaction rule).

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

const AUTH_USAGE: &str = "usage: pi-rust auth login <provider> [--api-key] [--no-browser]\n\n  --api-key        log in with an API key instead of an account\n  --no-browser     print sign-in URLs instead of opening a browser\n";

#[derive(Debug)]
enum AuthEvent {
    Write(String),
    Error(String),
    Prompt {
        id: String,
        secret: bool,
        message: String,
    },
    OpenUrl(String),
    Exit(i32),
}

/// Entry point for `pi-rust auth ...` (args after the `auth` word).
pub fn run(
    args: &[String],
    cwd: &Path,
    find_sidecar: &dyn Fn(&Path) -> Result<PathBuf, String>,
) -> i32 {
    let rest = match args.first().map(String::as_str) {
        Some("login") => &args[1..],
        _ => {
            eprint!("{AUTH_USAGE}");
            return 1;
        }
    };

    // Mirror upstream parseLoginRequest: one provider, then flags.
    let mut provider = String::new();
    let mut api_key = false;
    let mut no_browser = false;
    for arg in rest {
        if arg == "--api-key" {
            api_key = true;
        } else if arg == "--no-browser" {
            no_browser = true;
        } else if arg.starts_with('-') || !provider.is_empty() {
            eprint!("{AUTH_USAGE}");
            return 1;
        } else {
            provider = arg.clone();
        }
    }
    if provider.is_empty() {
        eprint!("{AUTH_USAGE}");
        return 1;
    }
    if api_key && no_browser {
        eprintln!("error: --no-browser only applies to account sign-in; omit it with --api-key.");
        return 1;
    }

    // Upstream requires an interactive terminal for login prompts.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("\"pi-rust auth login <provider>\" requires an interactive terminal");
        return 1;
    }

    let sidecar = match find_sidecar(cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fatal: {e}");
            return 1;
        }
    };
    let auth_script = sidecar
        .parent()
        .map(|d| d.join("auth-login.mjs"))
        .unwrap_or_else(|| sidecar.clone());
    let agent_dir = crate::config::resolve_agent_dir(
        None,
        std::env::var("PI_CODING_AGENT_DIR").ok().as_deref(),
        &std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".")),
    );
    if let Err(e) = crate::config::ensure_agent_dir(&agent_dir) {
        eprintln!("fatal: {e}");
        return 1;
    }

    let node = std::env::var("PI_RUST_NODE").unwrap_or_else(|_| "node".into());
    let mut cmd = Command::new(node);
    cmd.arg(&auth_script)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg(&provider)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if api_key {
        cmd.arg("--api-key");
    }
    if no_browser {
        cmd.arg("--no-browser");
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("fatal: starting the auth login helper failed: {e}");
            return 1;
        }
    };

    // Reader thread: child stdout lines -> events.
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel::<Result<AuthEvent, String>>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx.send(Err(format!("reading the auth login helper failed: {e}")));
                    return;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("malformed auth login line: {e}")));
                    return;
                }
            };
            let t = v
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let ev = match t {
                "write" => AuthEvent::Write(
                    v.get("line")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                "error" => AuthEvent::Error(
                    v.get("line")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                "prompt" => AuthEvent::Prompt {
                    id: v
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    secret: v.get("kind").and_then(serde_json::Value::as_str) == Some("secret"),
                    message: v
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
                "open_url" => AuthEvent::OpenUrl(
                    v.get("url")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                "exit" => AuthEvent::Exit(
                    v.get("code")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(1) as i32,
                ),
                other => {
                    let _ = tx.send(Err(format!("unknown auth login event type: {other}")));
                    return;
                }
            };
            if tx.send(Ok(ev)).is_err() {
                return; // main loop left
            }
        }
    });

    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut code = 1;
    for res in rx {
        let ev = match res {
            Ok(ev) => ev,
            Err(e) => {
                eprintln!("fatal: {e}");
                break;
            }
        };
        match ev {
            AuthEvent::Write(line) => println!("{line}"),
            AuthEvent::Error(line) => eprintln!("{line}"),
            AuthEvent::OpenUrl(url) => {
                // Best effort: the URL is already printed by the flow's own
                // notify; opening a browser is a convenience (upstream does).
                open_browser(&url);
            }
            AuthEvent::Prompt {
                id,
                secret,
                message,
            } => {
                let answer = if secret {
                    read_secret_line(&message)
                } else {
                    print!("{message}");
                    let _ = std::io::stdout().flush();
                    let mut buf = String::new();
                    match std::io::stdin().read_line(&mut buf) {
                        Ok(_) => buf,
                        Err(_) => String::new(),
                    }
                    .trim_end_matches(['\r', '\n'])
                    .to_string()
                };
                let _ = writeln!(
                    stdin,
                    "{{\"id\":\"{id}\",\"answer\":{}}}",
                    serde_json::json!(answer)
                );
                let _ = stdin.flush();
            }
            AuthEvent::Exit(c) => {
                code = c;
                break;
            }
        }
    }
    let _ = child.wait();
    code
}

/// Read one line without echoing (raw mode turns off ICANON+ECHO, the same
/// semantics as Node's `setRawMode` in the upstream secret reader). Ctrl+C
/// aborts the line (empty answer).
fn read_secret_line(message: &str) -> String {
    print!("{message}");
    let _ = std::io::stdout().flush();
    let mut raw = false;
    let ok = crossterm::terminal::enable_raw_mode();
    if ok.is_ok() {
        raw = true;
    }
    let mut value = String::new();
    let mut stdin = std::io::stdin();
    let mut byte = [0u8; 1];
    let mut csi = 0u8; // 0 = normal, remaining bytes of an escape sequence
    loop {
        match stdin.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                let b = byte[0];
                if csi > 0 {
                    // CSI ends at a final byte 0x40..=0x7E.
                    if (0x40..=0x7E).contains(&b) {
                        csi = 0;
                    }
                    continue;
                }
                match b {
                    b'\r' | b'\n' => break,
                    0x03 => {
                        // Ctrl+C: cancel the line.
                        value.clear();
                        break;
                    }
                    0x7f | 0x08 => {
                        value.pop();
                    }
                    0x1b => {
                        csi = 2; // at least ESC + one more byte
                    }
                    b if b >= 0x20 => {
                        value.push(b as char);
                    }
                    _ => {}
                }
            }
            Err(_) => break,
        }
    }
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    println!();
    value
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut cmd = Command::new(opener);
    if cfg!(target_os = "windows") {
        cmd.args(["/C", "start", "", url]);
    } else {
        cmd.arg(url);
    }
    let _ = cmd.spawn(); // best effort; the URL is already visible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_arg_parsing_rejects_bad_shapes() {
        // No provider -> usage, exit 1 (checked through run() only for the
        // arg-parse layer: it exits before any TTY/spawn work).
        assert_eq!(
            run(&[], Path::new("/tmp"), &|_| Err("no sidecar".into())),
            1
        );
        assert_eq!(
            run(&["login".into()], Path::new("/tmp"), &|_| Err(
                "no sidecar".into()
            )),
            1
        );
        assert_eq!(
            run(
                &[
                    "login".into(),
                    "xai".into(),
                    "--api-key".into(),
                    "--no-browser".into()
                ],
                Path::new("/tmp"),
                &|_| Err("no sidecar".into())
            ),
            1
        );
    }
}
