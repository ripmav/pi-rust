//! T4 end-to-end: the pi-rust core spawns the Node sidecar, loads UNMODIFIED
//! upstream example extensions (hello.ts, permission-gate.ts, with-deps/), and
//! runs a scenario whose scripted fake model (1) runs `bash echo bridge-ok`,
//! (2) calls the hello extension's `hello` tool, (3) triggers the
//! permission-gate block on `sudo true`. Asserts on the NDJSON event log.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Parse the NDJSON event log on a run's stdout; panics on a non-JSON line.
fn event_log(stdout: &str) -> Vec<Value> {
    stdout.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}")))
        .collect()
}

#[test]
fn bridge_scenario_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-t4-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let ext = repo().join("upstream/packages/coding-agent/examples/extensions");
    let scenario = json!({
        "user_messages": ["run the bridge checks"],
        "model_scripts": [[
            { "tool_call": { "name": "bash",  "input": { "command": "echo bridge-ok" } } },
            { "tool_call": { "name": "hello", "input": { "name": "bridge" } } },
            { "tool_call": { "name": "bash",  "input": { "command": "sudo true" } } },
            { "text": "all checks recorded" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust")) // hyphen kept: cargo names it after the bin target
        .arg("--extension-dir").arg(ext.join("hello.ts"))
        .arg("--extension-dir").arg(ext.join("permission-gate.ts"))
        .arg("--extension-dir").arg(ext.join("with-deps"))
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}")))
        .collect();

    // Every stdout line is a machine-readable event with a `kind`.
    assert!(lines.iter().all(|l| l.get("kind").is_some()), "non-event stdout: {stdout}");

    let tool_execs: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "tool_execution").collect();
    assert_eq!(tool_execs.len(), 3, "expected 3 tool executions\n{stdout}");

    let bash_ok = tool_execs.iter().find(|t| t["name"] == "bash" && t["blocked"] == json!(false)
        && t["input"]["command"] == "echo bridge-ok")
        .expect("bash echo bridge-ok missing\n{stdout}");
    assert!(bash_ok["result_summary"].as_str().unwrap().contains("bridge-ok"));

    let hello = tool_execs.iter().find(|t| t["name"] == "hello")
        .expect("hello extension tool call missing\n{stdout}");
    assert!(hello["result_summary"].as_str().unwrap().contains("Hello, bridge!"));

    let blocked = tool_execs.iter().find(|t| t["blocked"] == json!(true))
        .expect("blocked bash missing\n{stdout}");
    assert_eq!(blocked["input"]["command"], "sudo true");
    assert_eq!(blocked["result_summary"], "Blocked by extension: Blocked by user");

    // The tool_call decision for the blocked command carries the handler's block + reason.
    let decisions: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "decision").collect();
    let gate = decisions.iter().find(|d| d["name"] == "tool_call"
        && d["request"]["input"]["command"] == "sudo true")
        .expect("sudo tool_call decision missing\n{stdout}");
    assert_eq!(gate["response"]["result"]["block"], json!(true));
    assert_eq!(gate["response"]["result"]["reason"], "Blocked by user");

    // Turn boundaries and model outputs (turn 0 = 3 tool calls, turn 1 = text).
    assert!(lines.iter().any(|l| l["kind"] == "turn_start" && l["turnIndex"] == 0));
    assert!(lines.iter().any(|l| l["kind"] == "turn_end" && l["turnIndex"] == 1));
    let model_outputs: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "model_output").collect();
    assert_eq!(model_outputs.len(), 2);
    assert_eq!(model_outputs[0]["toolCalls"], json!(["bash", "hello", "bash"]));

    // Final transcript: full session = 1 user + 2 assistant + 3 tool results,
    // all as upstream v3 `message` entries.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries = last["session"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 6, "transcript entries wrong\n{stdout}");
    assert!(entries.iter().all(|e| e["type"] == "message"), "upstream v3 entry types: {entries:?}");
    let roles: Vec<&str> = entries.iter().map(|e| e["message"]["role"].as_str().unwrap()).collect();
    assert_eq!(roles, vec!["user", "assistant", "toolResult", "toolResult", "toolResult", "assistant"]);
    assert_eq!(last["session"]["messages"].as_array().unwrap().len(), 6);

    // The session JSONL file was written: v3 header + same 6 entries.
    let file_lines = std::fs::read_to_string(&session_file).unwrap().lines().count();
    assert_eq!(file_lines, 7);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// W1 GAP A: unmodified prompt-customizer.ts mutates systemPromptOptions.sections.
/// The core must send the BuildSystemPromptOptions subset (selectedTools incl. read)
/// and the sidecar must render the section via the upstream prompt builder.
#[test]
fn before_agent_start_system_prompt_options() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-w1-bas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let ext = repo().join("upstream/packages/coding-agent/examples/extensions");
    let scenario = json!({
        "user_messages": ["hello"],
        "model_scripts": [[{ "text": "done" }]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir").arg(ext.join("prompt-customizer.ts"))
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The handler ran cleanly: no extension_error line.
    assert!(
        !lines.iter().any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    // The core sends the BuildSystemPromptOptions subset with read selected.
    let bas = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("decision")
            && l.get("name").and_then(Value::as_str) == Some("before_agent_start"))
        .expect("before_agent_start decision missing\n{stdout}");
    let selected = bas["request"]["systemPromptOptions"]["selectedTools"]
        .as_array()
        .expect("request systemPromptOptions.selectedTools missing\n{bas}");
    assert!(selected.iter().any(|t| t == "read"), "core selectedTools must include read\n{bas}");

    // The response carries the rendered prompt with the tool_guidance section.
    let sp = bas["response"]["systemPrompt"]
        .as_str()
        .expect("rendered systemPrompt missing from before_agent_start response\n{stdout}");
    assert!(sp.contains("Use `read` for file contents"), "tool_guidance not rendered\n{sp}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// W1 GAP B: unmodified plan-mode/ driven by `--command plan` before the scenario:
/// destructive bash blocked, safe bash allowed, plan context injected, state
/// persisted by the core, agent_end select dialog answered with the v0 default.
#[test]
fn plan_mode_command_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-w1-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let ext = repo().join("upstream/packages/coding-agent/examples/extensions");
    let scenario = json!({
        "user_messages": ["Plan: 1. inspect 2. verify"],
        "model_scripts": [[
            { "tool_call": { "name": "bash", "input": { "command": "rm -rf /tmp/planmode-x" } } },
            { "tool_call": { "name": "bash", "input": { "command": "ls" } } },
            // Multi-line "Plan:" header: the unmodified extension's todo extractor
            // only matches numbered steps on their own lines.
            { "text": "Plan:\n1. inspect\n2. verify" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir").arg(ext.join("plan-mode"))
        .arg("--command").arg("plan")
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // (a) the command event was logged ok.
    let cmd = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("command"))
        .expect("command event missing\n{stdout}");
    assert_eq!(cmd["name"], "plan");
    assert_eq!(cmd["ok"], json!(true), "command event not ok\n{cmd}");

    // (b) the destructive tool_call is blocked with the plan-mode reason.
    let gate = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("decision")
            && l.get("name").and_then(Value::as_str) == Some("tool_call")
            && l["request"]["input"]["command"] == "rm -rf /tmp/planmode-x")
        .expect("rm tool_call decision missing\n{stdout}");
    assert_eq!(gate["response"]["result"]["block"], json!(true), "rm not blocked\n{gate}");
    let reason = gate["response"]["result"]["reason"].as_str().expect("block reason missing\n{gate}");
    assert!(reason.contains("Plan mode: command blocked"), "unexpected reason: {reason}");

    // (c) the safe bash executes (not blocked).
    assert!(
        lines.iter().any(|l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
            && l["name"] == "bash" && l["input"]["command"] == "ls" && l["blocked"] == json!(false)),
        "ls tool_execution missing or blocked\n{stdout}",
    );

    // (d) the plan-mode context message was injected for the run.
    let bas = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("decision")
            && l.get("name").and_then(Value::as_str) == Some("before_agent_start"))
        .expect("before_agent_start decision missing\n{stdout}");
    assert!(
        bas.to_string().contains("[PLAN MODE ACTIVE]"),
        "plan-mode context message missing\n{bas}",
    );

    // (e) the plan-mode state entry was persisted to the session file by the core.
    let session_text = std::fs::read_to_string(&session_file).expect("session file missing");
    assert!(
        session_text.lines().any(|l| l.contains("\"customType\":\"plan-mode\"")),
        "plan-mode custom entry missing from session file\n{session_text}",
    );

    // (f) the agent_end select dialog is logged and answered with the v0 default.
    let sel = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("extension_ui_request")
            && l.get("method").and_then(Value::as_str) == Some("select"))
        .expect("select extension_ui_request missing\n{stdout}");
    assert!(sel["title"].as_str().expect("select title missing").contains("Plan mode"), "{sel}");
    assert_eq!(
        sel["response"],
        json!({ "type": "extension_ui_response", "id": sel["id"], "cancelled": true }),
        "select not answered with v0 default\n{sel}",
    );

    // Healthy run: no handler errors.
    assert!(
        !lines.iter().any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// W1: nested tool execution — the fixture's ctx.executeTool routes via the
/// sidecar->core tool_call_request round-trip and the nested bash executes
/// with its normal hooks; the outer result carries the nested text.
#[test]
fn nested_tool_call_request_round_trip() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-w1-nest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let fixture = repo().join("tests/fixtures/nested-tool.ts");
    let scenario = json!({
        "user_messages": ["nest it"],
        "model_scripts": [[
            { "tool_call": { "name": "nest", "input": { "depth": 1 } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir").arg(&fixture)
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The nested bash crossed the bridge (tool_call_request round-trip) and executed.
    let nested = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
            && l["name"] == "bash" && l["input"]["command"] == "echo nested-ok" && l["blocked"] == json!(false))
        .expect("nested bash tool_execution missing\n{stdout}");
    assert!(nested["result_summary"].as_str().expect("nested summary missing").contains("nested-ok"), "{nested}");

    // The outer extension tool surfaced the nested result text.
    let outer = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("tool_execution") && l["name"] == "nest")
        .expect("nest tool_execution missing\n{stdout}");
    assert!(outer["result_summary"].as_str().expect("outer summary missing").contains("nested-ok"), "{outer}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// T5: unmodified claude-rules.ts scans <cwd>/.claude/rules on session_start
/// and appends the rule list to the system prompt on before_agent_start.
#[test]
fn claude_rules_system_prompt_injection() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-t5-claude-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".claude/rules")).unwrap();
    std::fs::write(proj.join(".claude/rules/testing.md"), "# Testing\nAlways run tests.\n").unwrap();

    let ext = repo().join("upstream/packages/coding-agent/examples/extensions");
    let scenario = json!({ "user_messages": ["hi"], "model_scripts": [[{ "text": "done" }]] });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    // claude-rules scans ctx.cwd/.claude/rules; the sidecar inherits the core's cwd.
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&proj)
        .arg("--extension-dir").arg(ext.join("claude-rules.ts"))
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    let bas = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("decision")
            && l.get("name").and_then(Value::as_str) == Some("before_agent_start"))
        .expect("before_agent_start decision missing\n{stdout}");
    let sp = bas["response"]["systemPrompt"]
        .as_str()
        .expect("rendered systemPrompt missing\n{bas}");
    assert!(sp.contains("## Project Rules"), "rules section missing\n{sp}");
    assert!(sp.contains(".claude/rules/testing.md"), "rule file missing\n{sp}");

    assert!(
        !lines.iter().any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// T5: sidecar crash mid extension_tool_call (test fixture kills the process)
/// -> core marks degraded: built-ins keep working, the transcript is still
/// emitted, and the run exits 3 (degraded) per the T4 exit-code table.
#[test]
fn sidecar_crash_degraded_continues_with_builtins() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-t5-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let fixture = repo().join("tests/fixtures/crash-tool.ts");
    let scenario = json!({
        "user_messages": ["crash then continue", "second message"],
        "model_scripts": [[
            { "tool_call": { "name": "crash", "input": {} } },
            { "text": "after crash" }
        ], [
            { "tool_call": { "name": "bash", "input": { "command": "echo post-crash" } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir").arg(&fixture)
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(3), "expected degraded exit 3\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // Built-in bash executes fine after the crash (degraded semantics).
    assert!(
        lines.iter().any(|l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
            && l["name"] == "bash" && l["input"]["command"] == "echo post-crash" && l["blocked"] == json!(false)),
        "post-crash bash tool_execution missing\n{stdout}",
    );

    // The run was marked degraded/transport-failed in the event log.
    assert!(
        lines.iter().any(|l| l.to_string().contains("degraded") || l.to_string().contains("sidecar")),
        "no degraded/sidecar marker in event log\n{stdout}",
    );

    // The transcript is still the final line.
    assert_eq!(lines.last().expect("no stdout lines")["kind"], "transcript");

    let _ = std::fs::remove_dir_all(&tmp);
}

// --- Milestone 2: real-provider path against a local mock (S1/S2 wiring) ---

#[test]
fn provider_e2e_prompt_via_mock_openai() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m2-provider-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // One-shot SSE mock on 127.0.0.1 (std thread; the child core hits it once).
    let c1 = json!({"choices":[{"delta":{"role":"assistant","content":"Hello"}}]}).to_string();
    let c2 = json!({"choices":[{"delta":{"content":" from mock"}}]}).to_string();
    let sse = format!("data: {c1}\ndata: {c2}\ndata: [DONE]\n");
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for line in sse.split_inclusive('\n') {
        if line.trim().is_empty() { continue; }
        let b = line.as_bytes();
        body.push_str(&format!("{:x}\r\n", b.len()));
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.push_str("0\r\n\r\n");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        use std::io::Read;
        let Ok((mut s, _)) = listener.accept() else { return };
        // Read until the header terminator appears anywhere in what we have:
        // reqwest delivers headers+body in one segment, so the tail check
        // must search the buffer, not only its last bytes.
        let mut buf = vec![0u8; 65536];
        let mut n = 0usize;
        loop {
            match s.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    n += k;
                    if n > 4 && buf[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                }
            }
        }
        use std::io::Write;
        let _ = s.write_all(body.as_bytes());
    });

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("OPENAI_API_KEY", "e2e-key")
        .env("OPENAI_BASE_URL", &base)
        .arg("--provider").arg("openai")
        .arg("--model").arg("mock-model")
        .arg("--prompt").arg("hi")
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");
    let _ = server.join();

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}")))
        .collect();
    let final_line = lines.last().expect("no events");
    assert_eq!(final_line["kind"], "transcript");
    let dump = final_line["session"].to_string();
    assert!(dump.contains("Hello from mock"), "transcript: {dump}");
}

/// WP-407: provider run with --print-usage emits a cumulative usage event
/// (cost from the bundled registry for a real model id) and writes a usage
/// entry into the session file.
#[test]
fn provider_e2e_print_usage_and_session_entry() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m4-usage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let c1 = json!({"choices":[{"delta":{"role":"assistant","content":"ok"}}]}).to_string();
    let c2 = json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":50}}).to_string();
    let sse = format!("data: {c1}\ndata: {c2}\ndata: [DONE]\n");
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for line in sse.split_inclusive('\n') {
        if line.trim().is_empty() { continue; }
        let b = line.as_bytes();
        body.push_str(&format!("{:x}\r\n", b.len()));
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.push_str("0\r\n\r\n");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        use std::io::Read;
        let Ok((mut s, _)) = listener.accept() else { return };
        let mut buf = vec![0u8; 65536];
        let mut n = 0usize;
        loop {
            match s.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    n += k;
                    if n > 4 && buf[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                }
            }
        }
        use std::io::Write;
        let _ = s.write_all(body.as_bytes());
    });

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("OPENAI_API_KEY", "e2e-key")
        .env("OPENAI_BASE_URL", &base)
        .arg("--provider").arg("openai")
        .arg("--model").arg("gpt-4o-mini")
        .arg("--prompt").arg("hi")
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(&session_file)
        .arg("--print-usage")
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");
    let _ = server.join();

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}")))
        .collect();
    let usage = lines
        .iter()
        .find(|l| l["kind"] == "usage")
        .expect("no usage event with --print-usage");
    assert_eq!(usage["input"], 100);
    assert_eq!(usage["output"], 50);
    // gpt-4o-mini registry cost: 0.15/1M in, 0.6/1M out -> 100*0.15 + 50*0.6 per 1M
    let expected = 100f64 / 1e6 * 0.15 + 50f64 / 1e6 * 0.6;
    assert!(
        (usage["costUsd"].as_f64().unwrap() - expected).abs() < 1e-12,
        "costUsd: {}", usage["costUsd"]
    );
    assert_eq!(lines.last().unwrap()["kind"], "transcript");

    let file = std::fs::read_to_string(&session_file).unwrap();
    let entry: Value = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e.get("type").and_then(Value::as_str) == Some("usage"))
        .expect("no usage entry in session file");
    assert_eq!(entry["usage"]["input"], 100);
    assert_eq!(entry["usage"]["output"], 50);
}

/// Looping SSE mock (same framing as the M2 e2e helper): accepts connections
/// until the process exits and always answers with the same body.
fn sse_loop_mock(text: &str) -> String {
    let c1 = json!({"choices":[{"delta":{"role":"assistant","content":text}}]}).to_string();
    let sse = format!("data: {c1}\ndata: [DONE]\n");
    let mut body = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for line in sse.split_inclusive('\n') {
        if line.trim().is_empty() { continue; }
        let b = line.as_bytes();
        body.push_str(&format!("{:x}\r\n", b.len()));
        body.push_str(line);
        body.push_str("\r\n");
    }
    body.push_str("0\r\n\r\n");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        loop {
            let Ok((mut s, _)) = listener.accept() else { break };
            let body_c = body.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 65536];
                let mut n = 0usize;
                loop {
                    match s.read(&mut buf[n..]) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => {
                            n += k;
                            if n > 4 && buf[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                        }
                    }
                }
                let _ = s.write_all(body_c.as_bytes());
            });
        }
    });
    base
}

/// WP-406: runtime setModel from an extension. The invalid id is declined
/// (no state change); after the core confirms the valid switch, a second
/// run (injected by the fixture) is served by the new driver. One
/// model_change entry, the declined id never recorded.
#[test]
fn provider_e2e_runtime_setmodel() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m4-setmodel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("agent")).unwrap();
    std::fs::write(
        tmp.join("agent").join("auth.json"),
        json!({"groq":{"type":"api_key","key":"groq-key"},
               "openai":{"type":"api_key","key":"openai-key"}}).to_string(),
    )
    .unwrap();

    // Looping mocks (the run is short; each mock may be hit 0..n times).
    // `before_agent_start` is a decision event the core awaits, so the
    // fixture's setModel calls are confirmed before the model call of the
    // same run — no race with the agent loop.
    let base_groq = sse_loop_mock("groq-text");
    let base_openai = sse_loop_mock("openai-text");

    // Two sequential runs: run 1 keeps the groq driver; the switch (confirmed
    // during run 2's before_agent_start) serves run 2's model call with the
    // new driver.
    let scenario = tmp.join("scenario.json");
    std::fs::write(&scenario, r#"{"user_messages":["first","second"]}"#).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setmodel.ts");
    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("GROQ_BASE_URL", &base_groq)
        .env("OPENAI_BASE_URL", &base_openai)
        .arg("--provider").arg("groq")
        .arg("--model").arg("llama-3.3-70b-versatile")
        .arg("--extension-dir").arg(&fixture)
        .arg("--scenario").arg(&scenario)
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}")))
        .collect();
    let dump = lines.last().unwrap()["session"].to_string();
    // Run 1 (prompt "first") was served by the original groq driver.
    assert!(dump.contains("groq-text"), "first run lost its driver: {dump}");
    // Run 2 (prompt "second") was served by the post-swap openai driver.
    assert!(dump.contains("second"), "second run missing: {dump}");
    assert!(dump.contains("openai-text"), "second run not served by openai: {dump}");

    // Exactly one model_change entry: the successful switch only.
    let file = std::fs::read_to_string(&session_file).unwrap();
    let changes: Vec<Value> = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("model_change"))
        .collect();
    assert_eq!(changes.len(), 1, "model_change entries: {file}");
    assert_eq!(changes[0]["provider"], "openai");
    assert_eq!(changes[0]["modelId"], "gpt-4o-mini");
    // The declined id must not appear anywhere in the session.
    assert!(!file.contains("nope-model"), "declined model leaked: {file}");
}

/// S4 cross-compatibility: a session file written by pi-rust must be loaded
/// by the ORIGINAL upstream SessionManager (read-only: open + read API).
#[test]
fn upstream_sessionmanager_reads_pirust_session() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-cross-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let ext = std::path::Path::new("tests/fixtures");
    let scenario = json!({
        "user_messages": ["cross-check session"],
        "model_scripts": [[
            { "tool_call": { "name": "bash", "input": { "command": "echo x" } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("cross.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir").arg(ext.join("hello.ts"))
        .arg("--agent-dir").arg(&agent_dir)
        .arg("--scenario").arg(&scen)
        .arg("--session-file").arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");

    // The original upstream SessionManager opens and reads the file.
    let node = std::env::var("PI_RUST_NODE").unwrap_or_else(|_| "node".into());
    let script = format!(r#"
import {{ SessionManager }} from "{dist}";
const sm = await SessionManager.open("{file}");
const header = sm.getHeader();
const entries = sm.getEntries();
const branch = sm.getBranch();
if (header.version !== 3) throw new Error("bad version " + header.version);
if (entries.length !== 4) throw new Error("bad entry count " + entries.length);
if (branch.length !== 4) throw new Error("bad branch length " + branch.length);
const roles = branch.filter(e => e.type === "message").map(e => e.message.role);
const want = ["user","assistant","toolResult","assistant"];
if (JSON.stringify(roles) !== JSON.stringify(want)) throw new Error("bad roles " + JSON.stringify(roles));
if (sm.getSessionFile() === null) throw new Error("no session file");
console.log("UPSTREAM-READ-OK");
"#,
        dist = format!("file://{}/upstream/packages/coding-agent/dist/index.js", env!("CARGO_MANIFEST_DIR")),
        file = session_file.to_string_lossy(),
    );
    let node_out = Command::new(&node)
        .arg("--input-type=module")
        .arg("--eval")
        .arg(script)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("running node");
    let node_stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let node_stderr = String::from_utf8_lossy(&node_out.stderr).to_string();
    assert!(node_stdout.contains("UPSTREAM-READ-OK"), "upstream SessionManager failed to read pi-rust session\n{node_stdout}\n{node_stderr}");
}

/// S5: --resume continues an existing session file (same id, chain grows).
#[test]
fn resume_appends_to_existing_session() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-resume-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent");
    let f = tmp.join("s.jsonl");
    let mk = |msgs: Vec<&str>| -> Value {
        json!({"user_messages": msgs, "model_scripts": msgs.iter().map(|m| vec![json!({"text": "ok: "})]).collect::<Vec<_>>()})
    };
    let s1 = tmp.join("s1.json");
    std::fs::write(&s1, mk(vec!["first"]).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent_dir).arg("--scenario").arg(&s1).arg("--session-file").arg(&f)
        .output().expect("run 1");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let id1: Value = serde_json::from_str(std::fs::read_to_string(&f).unwrap().lines().next().unwrap()).unwrap();
    let lines1 = std::fs::read_to_string(&f).unwrap().lines().count();
    assert_eq!(lines1, 3, "header + user + assistant");

    let s2 = tmp.join("s2.json");
    std::fs::write(&s2, mk(vec!["second"]).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent_dir).arg("--resume").arg(&f).arg("--scenario").arg(&s2)
        .output().expect("run 2");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let raw = std::fs::read_to_string(&f).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), lines1 + 2, "resume appends, never rewrites: {lines:?}");
    let h: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(h["id"], id1["id"], "resume keeps the session id");
    // Final transcript of run 2 contains BOTH messages.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let msgs = last["session"]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4, "transcript = 2 + 2 messages");
    let first_entry: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first_entry["parentId"].is_null(), true, "first entry has no parent");
    let prev: Value = serde_json::from_str(lines[lines1 - 1]).unwrap();
    let appended: Value = serde_json::from_str(lines[lines1]).unwrap();
    assert_eq!(appended["parentId"], prev["id"], "chain continues from the old leaf");
}

/// S5: --fork writes a NEW session (new id, parentSession = old id); the
/// original file is untouched.
#[test]
fn fork_creates_new_session_with_parent() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-fork-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent");
    let f = tmp.join("s.jsonl");
    let s1 = tmp.join("s1.json");
    std::fs::write(&s1, json!({"user_messages":["base"],"model_scripts":[[{"text":"base done"}]]}).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent_dir).arg("--scenario").arg(&s1).arg("--session-file").arg(&f)
        .output().expect("base run");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let base_raw = std::fs::read_to_string(&f).unwrap();
    let base_id: Value = serde_json::from_str(base_raw.lines().next().unwrap()).unwrap();

    let s2 = tmp.join("s2.json");
    std::fs::write(&s2, json!({"user_messages":["branch"],"model_scripts":[[{"text":"branch done"}]]}).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent_dir).arg("--fork").arg(&f).arg("--scenario").arg(&s2)
        .output().expect("fork run");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    // Original untouched.
    assert_eq!(std::fs::read_to_string(&f).unwrap(), base_raw, "fork must not modify the original");
    // Final transcript names the new file + parent id.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_ne!(last["session"]["id"], base_id["id"], "fork gets a new id");
    assert_eq!(last["session"]["parentSession"], base_id["id"], "fork records parentSession");
    let new_file = PathBuf::from(last["session"]["file"].as_str().unwrap());
    assert!(new_file.exists(), "fork file exists: {new_file:?}");
    let nh: Value = serde_json::from_str(std::fs::read_to_string(&new_file).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(nh["parentSession"], base_id["id"]);
}

/// S5: --list-sessions lists agent-dir sessions as NDJSON (no sidecar).
#[test]
fn list_sessions_lists_agent_sessions() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-list-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent");
    let s1 = tmp.join("s1.json");
    std::fs::write(&s1, json!({"user_messages":["one"],"model_scripts":[[{"text":"a"}]]}).to_string()).unwrap();
    for i in 1..=2 {
        let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
            .arg("--agent-dir").arg(&agent_dir).arg("--scenario").arg(&s1)
            .output().expect("run");
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent_dir).arg("--list-sessions")
        .output().expect("list");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "two sessions listed: {stdout}");
    for v in &lines {
        assert!(v.get("id").and_then(Value::as_str).is_some(), "entry has id: {v}");
        assert!(v.get("file").and_then(Value::as_str).is_some(), "entry has file: {v}");
    }
}

/// S6: default agent dir honors PI_CODING_AGENT_DIR (upstream env var);
/// --isolated forces a temp dir instead.
#[test]
fn agent_dir_default_env_and_isolated() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-agentdir-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let env_agent = tmp.join("env-agent");
    let s1 = tmp.join("s1.json");
    std::fs::write(&s1, json!({"user_messages":["a"],"model_scripts":[[{"text":"x"}]]}).to_string()).unwrap();
    let set_var = |c: &mut std::process::Command| {
        c.env("PI_CODING_AGENT_DIR", &env_agent);
    };

    // 1) env var default: no --agent-dir, PI_CODING_AGENT_DIR set.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    set_var(&mut cmd);
    let out = cmd.arg("--scenario").arg(&s1).output().expect("run with env agent dir");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let file = last["session"]["file"].as_str().unwrap().to_string();
    assert!(file.starts_with(env_agent.to_string_lossy().as_ref()), "session under env agent dir: {file}");

    // 2) --isolated: session file NOT under the env agent dir.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    set_var(&mut cmd);
    let out = cmd.arg("--isolated").arg("--scenario").arg(&s1).output().expect("run isolated");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let file = last["session"]["file"].as_str().unwrap().to_string();
    assert!(!file.starts_with(env_agent.to_string_lossy().as_ref()), "isolated run must not use the env agent dir: {file}");
    assert!(file.starts_with(std::env::temp_dir().to_string_lossy().as_ref()), "isolated run uses a temp dir: {file}");
}

/// S7: --template expands a discovered prompt template into the prompt
/// ($ARGUMENTS + positional), agent-dir discovery.
#[test]
fn template_expands_into_prompt() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-tpl-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent = tmp.join("agent");
    std::fs::create_dir_all(agent.join("prompts")).unwrap();
    std::fs::write(
        agent.join("prompts/greet.md"),
        "Greet $1 nicely, in $ARGUMENTS style",
    ).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&agent)
        .arg("--template").arg("greet").arg("World")
        .output()
        .expect("run with template");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "pi-rust exited non-zero\n{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let user: Vec<Value> = lines.iter().filter(|e| e["kind"] == "user_input").cloned().collect();
    assert_eq!(user.len(), 1, "one user message: {lines:?}");
    assert_eq!(user[0]["text"], "Greet World nicely, in World style");
}

/// S7: unknown template is a typed error, non-zero exit.
#[test]
fn template_unknown_is_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-tpl-err-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir").arg(&tmp)
        .arg("--template").arg("missing")
        .output()
        .expect("run with unknown template");
    assert_ne!(out.status.code(), Some(0), "unknown template must fail");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("missing"), "{stderr}");
}
