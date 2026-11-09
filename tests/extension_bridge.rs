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

    // Final transcript: full session = 1 user + 2 assistant + 3 tool results.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries = last["session"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 6, "transcript entries wrong\n{stdout}");
    let types: Vec<&str> = entries.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types, vec!["user", "assistant", "tool_result", "tool_result", "tool_result", "assistant"]);
    assert_eq!(last["session"]["messages"].as_array().unwrap().len(), 6);

    // The session JSONL file was written (same 6 entries).
    let file_lines = std::fs::read_to_string(&session_file).unwrap().lines().count();
    assert_eq!(file_lines, 6);

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
