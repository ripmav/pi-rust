//! WP-1205 e2e: the demo extension (`pi-ext-demo`, tool + command + event
//! handler + flag + shortcut + provider + MCP) running as a bridge-v6
//! process on stdio NDJSON. The harness speaks the v6 protocol directly —
//! **no Node sidecar is involved**: a Rust-only run speaks v6 end to end
//! (spawn → `init` → `ready` → dispatch → exit).
//!
//! Coverage (the WP-1205 acceptance list):
//! - the `init` handshake: `protocolVersion` validation, the `apiVersion`
//!   response, the `ready` catalog, the load-time MCP forward
//! - the dispatch loop: tool round-trip, a decision block, the
//!   `extension_ui_request`/`extension_ui_response` round-trip,
//!   `provider_stream` + `provider_stream_event` lines
//! - protocol errors: malformed line → parse error; unknown command type
//!   → `Unknown command` error (never a crash)
//! - panic safety: a panicking handler yields a clean error response /
//!   protocol default, never a half-line or a dead process

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);

/// The demo extension process with line-oriented stdio.
struct Proc {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
    rx: Receiver<String>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Proc {
    fn spawn_demo() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pi-ext-demo"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawning pi-ext-demo");
        let stdout = child.stdout.take().expect("stdout pipe");
        let stdin = child.stdin.take().expect("stdin pipe");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let _ = tx.send(l);
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            rx,
        }
    }

    fn send(&mut self, v: &Value) {
        let mut line = v.to_string();
        line.push('\n');
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin
            .write_all(line.as_bytes())
            .expect("write to demo stdin");
        stdin.flush().expect("flush demo stdin");
    }

    fn send_raw(&mut self, s: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(s.as_bytes()).expect("write to demo stdin");
        stdin.flush().expect("flush demo stdin");
    }

    /// Close stdin (the core dropped the pipe → clean exit 0).
    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    /// One stdout line, parsed (the reader thread never sends blanks).
    fn next_line(&self, what: &str) -> Value {
        let line = self
            .rx
            .recv_timeout(TIMEOUT)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("stdout line not JSON: {line}: {e}"))
    }

    /// Lines until (and including) the first line matching `pred`.
    fn lines_until(&self, mut pred: impl FnMut(&Value) -> bool, what: &str) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            let v = self.next_line(what);
            let done = pred(&v);
            out.push(v);
            if done {
                return out;
            }
        }
    }

    /// The init handshake; returns the `ready` line.
    fn init(&mut self, ui: &[&str], timeout_ms: u64) -> Value {
        self.send(&json!({
            "id": "init-1",
            "type": "init",
            "protocolVersion": 6,
            "capabilities": { "ui": ui, "timeoutMs": timeout_ms },
        }));
        let resp = self.next_line("init response");
        assert_eq!(resp["id"], "init-1", "init response id: {resp}");
        assert_eq!(resp["success"], json!(true), "init failed: {resp}");
        assert_eq!(resp["data"]["apiVersion"], json!(0), "apiVersion: {resp}");
        let ready = self.next_line("ready");
        assert_eq!(ready["type"], "ready", "ready first: {ready}");
        ready
    }

    /// Wait for the process to exit; expect `code`.
    fn wait_exit(&mut self, code: i32) {
        let status = self.child.wait().expect("waiting for demo");
        assert_eq!(status.code(), Some(code), "exit code: {status}");
    }
}

/// Answer the demo's load-time MCP forward so its request resolves fast
/// (instead of burning the capability timeout).
fn answer_mcp_forward(p: &mut Proc) {
    let line = p
        .lines_until(
            |v| v.get("type").and_then(Value::as_str) == Some("mcp_server_register"),
            "mcp_server_register request",
        )
        .pop()
        .unwrap();
    assert_eq!(line["name"], "docs");
    let id = line["id"].as_str().expect("request id").to_string();
    p.send(&json!({
        "id": id,
        "type": "response",
        "command": "mcp_server_register",
        "success": true,
        "data": { "ok": true },
    }));
}

/// Handshake + catalog: tools, commands, shortcut, flag, provider, events,
/// the v5 active tool set, and the load-time MCP forward.
#[test]
fn ready_catalog_from_stored_registrations() {
    let mut p = Proc::spawn_demo();
    let ready = p.init(&["confirm"], 2000);

    assert_eq!(ready["apiVersion"], json!(0));
    assert_eq!(
        ready["extensions"][0]["path"].as_str().unwrap(),
        env!("CARGO_BIN_EXE_pi-ext-demo")
    );

    let tool_names: Vec<&str> = ready["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tool_names, vec!["hello", "ask"]);
    let hello = &ready["tools"][0];
    assert_eq!(hello["description"], "Say hello to someone");
    assert_eq!(hello["parameters"]["properties"]["name"]["type"], "string");
    assert_eq!(hello["parameters"]["required"], json!(["name"]));

    let cmd_names: Vec<&str> = ready["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(cmd_names, vec!["greet", "boom"]);

    assert_eq!(ready["shortcuts"][0]["key"], "ctrl+d");
    assert_eq!(ready["flags"][0]["name"], "greet");
    assert_eq!(ready["flags"][0]["type"], "string");

    assert_eq!(ready["providers"][0]["name"], "faketest");
    assert_eq!(ready["providers"][0]["models"][0]["id"], "echo-1");
    assert_eq!(ready["providers"][0]["models"][0]["provider"], "faketest");

    let events: Vec<&str> = ready["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert_eq!(events, vec!["session_start", "tool_call"]);

    // v5 active tool set: both tools are direct + default-active.
    assert_eq!(ready["activeTools"], json!(["hello", "ask"]));

    // Load-time MCP registration forwarded to the core as a v6 request.
    answer_mcp_forward(&mut p);
}

/// The `extension_tool_call` round-trip: the demo's `hello` tool result
/// comes back as `data` (the `AgentToolResult` JSON).
#[test]
fn extension_tool_call_round_trip() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "tc-1",
        "type": "extension_tool_call",
        "toolCallId": "outer-1",
        "toolName": "hello",
        "input": { "name": "World" },
    }));
    let resp = p.lines_until(
        |v| v.get("type").and_then(Value::as_str) == Some("response") && v["id"] == "tc-1",
        "extension_tool_call response",
    );
    let resp = resp.last().unwrap();
    assert_eq!(resp["success"], json!(true), "{resp}");
    let text = resp["data"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, "Hello, World!");

    // An unknown tool name is a clean error response, not a crash.
    p.send(&json!({
        "id": "tc-2",
        "type": "extension_tool_call",
        "toolCallId": "outer-2",
        "toolName": "nope",
        "input": {},
    }));
    let resp = p.next_line("unknown tool response");
    assert_eq!(resp["id"], "tc-2");
    assert_eq!(resp["success"], json!(false));
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("Unknown extension tool: nope"));
}

/// The `tool_call` decision: the demo blocks `dangerous` calls; the
/// response carries `{ result, input }` (input echoed unchanged).
#[test]
fn tool_call_decision_block() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "dec-1",
        "type": "tool_call",
        "event": {
            "type": "toolCall",
            "toolCallId": "tc-9",
            "toolName": "bash",
            "input": { "dangerous": true },
        },
    }));
    let resp = p.next_line("tool_call response");
    assert_eq!(resp["id"], "dec-1");
    assert_eq!(resp["success"], json!(true), "{resp}");
    assert_eq!(resp["data"]["result"]["block"], json!(true), "{resp}");
    assert!(resp["data"]["result"]["reason"]
        .as_str()
        .unwrap()
        .contains("blocked"));
    assert_eq!(
        resp["data"]["input"]["dangerous"],
        json!(true),
        "input echoed: {resp}"
    );

    // A non-dangerous call is not blocked (handler returns None → null).
    p.send(&json!({
        "id": "dec-2",
        "type": "tool_call",
        "event": {
            "type": "toolCall",
            "toolCallId": "tc-10",
            "toolName": "bash",
            "input": { "command": "ls" },
        },
    }));
    let resp = p.next_line("tool_call response 2");
    assert_eq!(resp["id"], "dec-2");
    assert_eq!(resp["data"]["result"], json!(null), "{resp}");
    assert_eq!(resp["data"]["input"]["command"], "ls");
}

/// The `extension_ui_request`/`extension_ui_response` round-trip: the
/// demo's `ask` tool opens a confirm dialog; the harness answers.
#[test]
fn ui_dialog_round_trip() {
    let mut p = Proc::spawn_demo();
    p.init(&["confirm"], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "ask-1",
        "type": "extension_tool_call",
        "toolCallId": "outer-3",
        "toolName": "ask",
        "input": {},
    }));
    let req = p.next_line("extension_ui_request");
    assert_eq!(req["type"], "extension_ui_request");
    assert_eq!(req["method"], "confirm");
    assert_eq!(req["title"], "Proceed?");
    let dialog_id = req["id"].as_str().expect("dialog id").to_string();

    p.send(&json!({
        "id": dialog_id,
        "type": "extension_ui_response",
        "confirmed": true,
    }));
    let resp = p.next_line("ask response");
    assert_eq!(resp["id"], "ask-1");
    assert_eq!(resp["success"], json!(true), "{resp}");
    assert_eq!(resp["data"]["content"][0]["text"], "confirmed");
}

/// Headless (no `ui` methods advertised): the confirm resolves to the
/// upstream default (`false`) without emitting a request.
#[test]
fn ui_unsupported_resolves_default() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "ask-2",
        "type": "extension_tool_call",
        "toolCallId": "outer-4",
        "toolName": "ask",
        "input": {},
    }));
    // No extension_ui_request may appear: the next line is the response.
    let resp = p.next_line("ask response");
    assert_eq!(resp["id"], "ask-2");
    assert_eq!(resp["success"], json!(true), "{resp}");
    assert_eq!(resp["data"]["content"][0]["text"], "declined");
}

/// `provider_stream`: the ack, then one `provider_stream_event` line per
/// upstream event, terminated by `done`.
#[test]
fn provider_stream_events() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "ps-1",
        "type": "provider_stream",
        "streamId": "s1",
        "model": { "provider": "faketest", "id": "echo-1" },
        "messages": [{ "role": "user", "content": "ping" }],
        "options": {},
    }));
    let resp = p.next_line("provider_stream ack");
    assert_eq!(resp["id"], "ps-1");
    assert_eq!(resp["success"], json!(true), "{resp}");
    assert_eq!(resp["data"]["ok"], json!(true));
    assert_eq!(resp["data"]["streamId"], "s1");

    let mut deltas: Vec<String> = Vec::new();
    let mut seen_done = false;
    for _ in 0..10 {
        let line = p.next_line("provider_stream_event");
        assert_eq!(line["type"], "provider_stream_event");
        assert_eq!(line["streamId"], "s1");
        let et = line["event"]["type"].as_str().unwrap();
        if et == "text_delta" {
            deltas.push(line["event"]["delta"].as_str().unwrap().to_string());
        }
        if et == "done" {
            seen_done = true;
            assert_eq!(line["event"]["reason"], "stop");
            assert_eq!(line["event"]["message"]["content"][0]["text"], "echo: ping");
            break;
        }
    }
    assert!(seen_done, "terminal done missing");
    assert_eq!(deltas, vec!["echo: ping".to_string()]);

    // An unknown provider is a clean ack error (no stream).
    p.send(&json!({
        "id": "ps-2",
        "type": "provider_stream",
        "streamId": "s2",
        "model": { "provider": "nope", "id": "x" },
        "messages": [],
        "options": {},
    }));
    let resp = p.next_line("unknown provider ack");
    assert_eq!(resp["id"], "ps-2");
    assert_eq!(resp["success"], json!(false));
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("unknown extension provider: nope"));
}

/// Malformed line → the protocol parse error (`id: null`,
/// `command: "parse"`); the process keeps serving.
#[test]
fn malformed_line_parse_error() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send_raw("this is not json\n");
    let resp = p.next_line("parse error");
    assert_eq!(resp["id"], json!(null));
    assert_eq!(resp["command"], "parse");
    assert_eq!(resp["success"], json!(false));
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .starts_with("Failed to parse command"));
    // The process is alive: a command still round-trips.
    p.send(&json!({ "id": "g1", "type": "command", "name": "greet", "args": "x" }));
    let resp = p.next_line("command response");
    assert_eq!(resp["id"], "g1");
    assert_eq!(resp["success"], json!(true));
    assert_eq!(resp["data"]["args"], "x");
}

/// Unknown command `type` → the protocol error response (never a crash);
/// the process keeps serving.
#[test]
fn unknown_command_type_protocol_error() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({ "id": "u1", "type": "frobnicate" }));
    let resp = p.next_line("unknown command response");
    assert_eq!(resp["id"], "u1");
    assert_eq!(resp["success"], json!(false));
    assert_eq!(resp["error"], "Unknown command: frobnicate");
    // Still alive.
    p.send(&json!({ "id": "g2", "type": "command", "name": "greet", "args": "y" }));
    let resp = p.next_line("command response 2");
    assert_eq!(resp["success"], json!(true));
}

/// A command before `init` is refused (the sidecar's guard).
#[test]
fn command_before_init_refused() {
    let mut p = Proc::spawn_demo();
    p.send(&json!({ "id": "b1", "type": "command", "name": "greet", "args": "" }));
    let resp = p.next_line("pre-init response");
    assert_eq!(resp["id"], "b1");
    assert_eq!(resp["success"], json!(false));
    assert!(resp["error"].as_str().unwrap().contains("not initialized"));
}

/// Wrong `protocolVersion` → init error + non-zero exit.
#[test]
fn wrong_protocol_version_rejected() {
    let mut p = Proc::spawn_demo();
    p.send(&json!({
        "id": "init-9",
        "type": "init",
        "protocolVersion": 5,
        "capabilities": { "ui": [], "timeoutMs": 2000 },
    }));
    let resp = p.next_line("init rejection");
    assert_eq!(resp["id"], "init-9");
    assert_eq!(resp["success"], json!(false));
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("Unsupported protocolVersion: 5"));
    p.wait_exit(1);
}

/// Panic safety: a panicking command handler → a clean error response
/// (the message, not a half-line or a crash); the process keeps serving.
#[test]
fn panicking_command_clean_error_response() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({ "id": "p1", "type": "command", "name": "boom", "args": "" }));
    let resp = p.next_line("boom response");
    assert_eq!(resp["id"], "p1");
    assert_eq!(resp["success"], json!(false));
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("demo command panic"),
        "{resp}"
    );
    // The process survived: the next command works.
    p.send(&json!({ "id": "g3", "type": "command", "name": "greet", "args": "z" }));
    let resp = p.next_line("post-panic command");
    assert_eq!(resp["id"], "g3");
    assert_eq!(resp["success"], json!(true));
}

/// Panic safety: a panicking `tool_call` decision handler → a typed
/// `extension_error` + the protocol default (no block, input unchanged).
#[test]
fn panicking_tool_call_decision_default() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "p2",
        "type": "tool_call",
        "event": {
            "type": "toolCall",
            "toolCallId": "tc-p",
            "toolName": "bash",
            "input": { "panic": true },
        },
    }));
    let lines = p.lines_until(
        |v| v.get("type").and_then(Value::as_str) == Some("response") && v["id"] == "p2",
        "panicking tool_call response",
    );
    let err = lines
        .iter()
        .find(|v| v.get("type").and_then(Value::as_str) == Some("extension_error"))
        .expect("extension_error line");
    assert_eq!(err["event"], "tool_call");
    assert!(err["error"]
        .as_str()
        .unwrap()
        .contains("demo tool_call panic"));
    let resp = lines.last().unwrap();
    assert_eq!(
        resp["success"],
        json!(true),
        "decision resolves to its default"
    );
    assert_eq!(resp["data"]["result"], json!(null));
    assert_eq!(
        resp["data"]["input"]["panic"],
        json!(true),
        "input unchanged"
    );
    // Still alive.
    p.send(&json!({ "id": "g4", "type": "command", "name": "greet", "args": "w" }));
    let resp = p.next_line("post-panic command 2");
    assert_eq!(resp["success"], json!(true));
}

/// Fire-and-forget event: `session_start` (the demo declares it in
/// `events()` and logs it) gets no response; the process keeps serving.
#[test]
fn session_start_fire_and_forget() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "ev-1",
        "type": "session_start",
        "event": { "type": "sessionStart", "reason": "startup" },
    }));
    // No response to a F&F event: the next line is the answer to the
    // command sent after it.
    p.send(&json!({ "id": "g5", "type": "command", "name": "greet", "args": "f&f" }));
    let resp = p.next_line("command after session_start");
    assert_eq!(resp["id"], "g5", "F&F event must not produce a response");
    assert_eq!(resp["success"], json!(true));
    assert_eq!(resp["data"]["args"], "f&f");
}

/// `session_shutdown` → clean exit 0.
#[test]
fn session_shutdown_exits_zero() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.send(&json!({
        "id": "sh-1",
        "type": "session_shutdown",
        "event": { "type": "sessionShutdown", "reason": "user" },
    }));
    p.wait_exit(0);
}

/// stdin EOF (the core dropped the pipe) → clean exit 0.
#[test]
fn stdin_eof_exits_zero() {
    let mut p = Proc::spawn_demo();
    p.init(&[], 2000);
    answer_mcp_forward(&mut p);
    p.close_stdin();
    p.wait_exit(0);
}
