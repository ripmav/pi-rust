use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// NDJSON framing over the sidecar's stdio (docs/bridge-protocol.md §Framing):
/// one JSON object per line, `\n`-delimited, trailing `\r` stripped (tokio's
/// `lines()` mirrors the upstream jsonl semantics). Both directions run as
/// separate tasks — no head-of-line blocking.
pub async fn spawn(
    node: &str,
    sidecar: &Path,
    sidecar_args: &[String],
    cwd: &Path,
    stderr: Stdio,
) -> Result<(Child, mpsc::UnboundedSender<String>, mpsc::UnboundedReceiver<Value>)> {
    let mut child = Command::new(node)
        .arg(sidecar)
        .args(sidecar_args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("spawning sidecar: {node} {}", sidecar.display()))?;
    let stdin = child.stdin.take().context("sidecar stdin")?;
    let stdout = child.stdout.take().context("sidecar stdout")?;

    // Writer: one line per command. A dead pipe ends the loop; the read side's
    // EOF then surfaces it as a transport error (protocol rule).
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut w = stdin;
        while let Some(line) = rx.recv().await {
            if w.write_all(line.as_bytes()).await.is_err() || w.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = w.flush().await;
        }
        let _ = w.shutdown().await;
    });

    // Reader: parsed sidecar lines out; channel close = stdout EOF.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => {
                    if out_tx.send(v).is_err() {
                        break;
                    }
                }
                Err(e) => crate::corelog::line(&format!("[core] dropping unreadable sidecar line: {e}")),
            }
        }
    });

    Ok((child, tx, out_rx))
}
