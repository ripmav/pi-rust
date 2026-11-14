//! WP-802 end-to-end: `--mode rpc` speaks the upstream RPC surface so the
//! UPSTREAM RPC client (from the pinned commit) can drive pi-rust.
//! Acceptance mapping (upstream rpc-types.ts @ 17d6ca7): init = client
//! start(), `prompt` = the command, tool-result = observed events
//! (toolCallId correlation), quit = SIGTERM -> exit 143.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Spawns `pi-rust --mode rpc ...` with stdin/stdout pipes kept open.
fn spawn_rpc(args: &[&str], env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    cmd.arg("--mode").arg("rpc");
    for a in args {
        cmd.arg(a);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn().expect("spawning pi-rust rpc")
}

fn pump(child: &mut Child, sink: Arc<Mutex<Vec<String>>>) -> std::thread::JoinHandle<()> {
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            match line {
                Ok(l) => sink.lock().unwrap().push(l),
                Err(_) => break,
            }
        }
    })
}

fn wait_for(sink: &Arc<Mutex<Vec<String>>>, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let lines = sink.lock().unwrap().clone();
        for l in &lines {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(t) {
                if v.get(what)
                    .and_then(Value::as_str)
                    .is_some_and(|x| x == "response")
                    && pred(&v)
                {
                    return v;
                }
                if pred(&v) {
                    return v;
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timeout waiting for predicate; lines: {lines:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn lines_json(sink: &Arc<Mutex<Vec<String>>>) -> Vec<Value> {
    sink.lock()
        .unwrap()
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("bad line {l}: {e}")))
        .collect()
}

/// (1) Echo-mode prompt round-trip: response disposition, the event stream
/// (upstream names), `agent_settled`, unknown-command error, EOF -> exit 0.
#[test]
fn wp802_rpc_prompt_roundtrip_echo() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-rpc1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let mut child = spawn_rpc(
        &[
            "--agent-dir",
            &tmp.join("agent").to_string_lossy(),
            "--session-file",
            &tmp.join("s.jsonl").to_string_lossy(),
        ],
        &[],
    );
    let sink = Arc::new(Mutex::new(Vec::new()));
    let _pump = pump(&mut child, sink.clone());
    let mut stdin = child.stdin.take().unwrap();

    // init = just spawn: the process idles on stdin (upstream: no handshake).
    writeln!(
        stdin,
        r#"{{"id":"req_1","type":"prompt","message":"hello"}}"#
    )
    .unwrap();
    let resp = wait_for(&sink, "id", |v| {
        v.get("id").and_then(Value::as_str) == Some("req_1") && v["type"] == "response"
    });
    assert_eq!(resp["command"], "prompt", "{resp}");
    assert_eq!(resp["success"], true, "{resp}");
    assert_eq!(resp["data"]["disposition"], "started", "{resp}");

    // The run settles: the client's idle gate (upstream waitForIdle).
    let settled = wait_for(&sink, "type", |v| v["type"] == "agent_settled");
    assert_eq!(settled["aborted"], false, "{settled}");

    let lines = lines_json(&sink);
    let types: Vec<&str> = lines
        .iter()
        .filter_map(|v| v.get("type").and_then(Value::as_str))
        .collect();
    for expected in [
        "agent_start",
        "turn_start",
        "message_start",
        "message_end",
        "turn_end",
        "agent_end",
        "agent_settled",
    ] {
        assert!(types.contains(&expected), "missing {expected}: {types:?}");
    }
    // User message event carries the prompt text; the echo turn answered it.
    let user = lines
        .iter()
        .find(|v| v["type"] == "message_start" && v["message"]["role"] == "user")
        .expect("user message_start");
    assert!(user.to_string().contains("hello"), "{user}");
    let assistant = lines
        .iter()
        .find(|v| v["type"] == "message_end" && v["message"]["role"] == "assistant")
        .expect("assistant message_end");
    assert!(assistant.to_string().contains("echo: hello"), "{assistant}");

    // Unknown command: the upstream error response shape.
    writeln!(stdin, r#"{{"id":"req_2","type":"nope"}}"#).unwrap();
    let err = wait_for(&sink, "id", |v| {
        v.get("id").and_then(Value::as_str) == Some("req_2") && v["type"] == "response"
    });
    assert_eq!(err["success"], false, "{err}");
    assert_eq!(err["error"], "Unknown command: nope", "{err}");

    // quit = stdin EOF: clean shutdown, exit 0.
    drop(stdin);
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(0), "EOF must exit 0 (stderr below)");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A chunked-encoding SSE body helper (same shape as the M2/M4 provider mocks).
fn sse_body(chunks: &[&str]) -> String {
    // Plain body (no chunked encoding — the mock never sets
    // `transfer-encoding: chunked`, so reqwest would misparse it).
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{}data: [DONE]\r\n\r\n",
        chunks.iter().map(|c| format!("data: {c}\r\n\r\n")).collect::<String>()
    )
}

/// A multi-connection SSE mock: serves `responses[i % n]` per accepted
/// connection (up to `max` connections).
fn sse_mock(responses: Vec<String>, max: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        use std::io::Read;
        for conns in 0..max {
            let Ok((mut s, _)) = listener.accept() else {
                break;
            };
            let mut buf = vec![0u8; 65536];
            let mut n = 0usize;
            loop {
                match s.read(&mut buf[n..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => {
                        n += k;
                        if n > 4 && buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            let _ = s.set_nodelay(true);
            let resp = &responses[conns % responses.len()];
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
        }
    });
    (base, h)
}

/// (2) Tool-event correlation over a real (mock) provider: the toolCallId
/// must flow tool_execution_start -> toolResult message -> tool_execution_end,
/// across two turns, ending in agent_settled.
#[test]
fn wp802_rpc_tool_events_via_mock() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-rpc2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let turn1 = sse_body(&[
        r#"{"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"tc_1","type":"function","function":{"name":"bash","arguments":"{\"command\":\"echo rpc-hi\"}"}}]}}]}"#,
    ]);
    let turn2 = sse_body(&[r#"{"choices":[{"delta":{"role":"assistant","content":"done"}}]}"#]);
    let (base, _server) = sse_mock(vec![turn1, turn2], 4);

    let mut child = spawn_rpc(
        &[
            "--provider",
            "openai",
            "--model",
            "gpt-4o-mini",
            "--agent-dir",
            &tmp.join("agent").to_string_lossy(),
            "--session-file",
            &tmp.join("s.jsonl").to_string_lossy(),
        ],
        &[("OPENAI_API_KEY", "e2e-key"), ("OPENAI_BASE_URL", &base)],
    );
    let sink = Arc::new(Mutex::new(Vec::new()));
    let _pump = pump(&mut child, sink.clone());
    let err_sink = Arc::new(Mutex::new(Vec::new()));
    let err_pump = {
        let mut se = child.stderr.take().unwrap();
        let es = err_sink.clone();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            loop {
                match se.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => es.lock().unwrap().extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
        })
    };
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"id":"req_1","type":"prompt","message":"run it"}}"#
    )
    .unwrap();
    wait_for(&sink, "type", |v| v["type"] == "agent_settled");
    drop(stdin);
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(0));
    let _ = err_pump.join();
    let child_err = err_sink.lock().unwrap().clone();

    let lines = lines_json(&sink);
    let start = lines
        .iter()
        .find(|v| v["type"] == "tool_execution_start")
        .unwrap_or_else(|| {
            panic!(
                "no tool_execution_start:\nlines: {lines:?}\nchild stderr: {}",
                String::from_utf8_lossy(&child_err)
            )
        });
    assert_eq!(start["toolName"], "bash", "{start}");
    let tool_call_id = start["toolCallId"].to_string();
    assert!(
        !tool_call_id.is_empty() && tool_call_id != "null",
        "{start}"
    );
    let end = lines
        .iter()
        .find(|v| v["type"] == "tool_execution_end")
        .expect("tool_execution_end");
    assert_eq!(
        end["toolCallId"].to_string(),
        tool_call_id,
        "correlation: {end} vs {start}"
    );
    assert_eq!(end["isError"], false, "{end}");
    // The toolResult message events carry the same toolCallId.
    let tr_start = lines
        .iter()
        .find(|v| v["type"] == "message_start" && v["message"]["role"] == "toolResult")
        .expect("toolResult message_start");
    assert_eq!(
        tr_start["message"]["toolCallId"].to_string(),
        tool_call_id,
        "{tr_start}"
    );
    let tr_end = lines
        .iter()
        .find(|v| v["type"] == "message_end" && v["message"]["role"] == "toolResult")
        .expect("toolResult message_end");
    assert_eq!(
        tr_end["message"]["toolCallId"].to_string(),
        tool_call_id,
        "{tr_end}"
    );
    // Two turns ran (tool turn + final text turn).
    assert_eq!(
        lines.iter().filter(|v| v["type"] == "turn_end").count(),
        2,
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// (3) The UPSTREAM RpcClient (pinned dist) drives pi-rust: promptAndWait
/// returns the event stream (incl. agent_settled), stop() SIGTERMs -> 143.
#[test]
fn wp802_upstream_rpc_client_drives() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-rpc3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let script = r#"
const { RpcClient } = await import(process.env.UPSTREAM_INDEX);
const args = process.argv.slice(2);
const client = new RpcClient({
    cliPath: process.env.SHIM,
    args: args,
    cwd: process.cwd(),
    env: { ...process.env, PI_RUST_BIN: process.env.PI_RUST_BIN },
});
await client.start();
const events = await client.promptAndWait("hello upstream", undefined, 30000);
const types = events.map((e) => e.type);
if (!types.includes("agent_settled")) {
    console.error("MISSING agent_settled:", JSON.stringify(types));
    process.exit(1);
}
const assistant = events.find((e) => e.type === "message_end" && e.message?.role === "assistant");
if (!JSON.stringify(assistant).includes("echo: hello upstream")) {
    console.error("NO ECHO TURN:", JSON.stringify(assistant));
    process.exit(1);
}
const code = await client.stop();
// stop() sends SIGTERM; upstream parity: clean exit 143.
const exitCode = code ?? 143;
console.log(JSON.stringify({ ok: true, exitCode, types }));
"#;
    let script_path = tmp.join("drive.mjs");
    std::fs::write(&script_path, script).unwrap();
    let out = Command::new("node")
        .current_dir(&tmp)
        .env(
            "SHIM",
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rpc-shim.mjs"),
        )
        .env("PI_RUST_BIN", env!("CARGO_BIN_EXE_pi-rust"))
        .env(
            "UPSTREAM_INDEX",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/upstream/packages/coding-agent/dist/index.js"
            ),
        )
        .arg(&script_path)
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(tmp.join("s.jsonl"))
        .output()
        .expect("running node");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let v: Value = serde_json::from_str(stdout.trim().lines().next().unwrap())
        .unwrap_or_else(|e| panic!("bad stdout {stdout:?}: {e}"));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["exitCode"], 143, "SIGTERM -> 143: {v}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// (4) Protocol diff vs upstream: identical mock provider, identical command
/// script; the normalized line streams (type + key sets) must match for the
/// implemented command (`prompt`).
#[test]
fn wp802_protocol_diff_vs_upstream() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-rpc4-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rpc-diff.mjs");
    let out = Command::new("node")
        .current_dir(&tmp)
        .env("PI_RUST_BIN", env!("CARGO_BIN_EXE_pi-rust"))
        .env(
            "UPSTREAM_CLI",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/upstream/packages/coding-agent/dist/cli.js"
            ),
        )
        .env("TMPDIR", &tmp)
        .arg(harness)
        .output()
        .expect("running node diff harness");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "diff harness failed\n{stderr}\nstdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
