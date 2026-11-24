//! T4 end-to-end: the pi-rust core spawns the Node sidecar, loads UNMODIFIED
//! upstream example extensions (hello.ts, permission-gate.ts, with-deps/), and
//! runs a scenario whose scripted fake model (1) runs `bash echo bridge-ok`,
//! (2) calls the hello extension's `hello` tool, (3) triggers the
//! permission-gate block on `sudo true`. Asserts on the NDJSON event log.

use serde_json::{json, Value};

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// WP-610 golden: `--print` (text mode) — stdout is the final assistant
/// text only (one trailing newline), no NDJSON event log, exit 0.
#[test]
fn wp610_print_mode_text_golden() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp610-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let scenario = json!({
        "user_messages": ["run the checks"],
        "model_scripts": [[
            { "tool_call": { "name": "bash", "input": { "command": "echo ok" } } },
            { "text": "all good" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--print")
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(stdout, "all good\n", "exact text-mode stdout: {stdout:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-610 golden: `--mode json` — stdout is the upstream-shaped
/// AgentSessionEvent stream (header first), exit 0.
#[test]
fn wp610_json_mode_event_stream_golden() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp610j-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let scenario = json!({
        "user_messages": ["run the checks"],
        "model_scripts": [[
            { "tool_call": { "name": "bash", "input": { "command": "echo ok" } } },
            { "text": "all good" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--mode")
        .arg("json")
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l}: {e}")))
        .collect();
    // 1. session header; 2. agent_start; 3. turn_start.
    // Upstream order: the session header, the user message (added on
    // prompt), then the agent loop.
    assert_eq!(
        lines[0].get("type").and_then(Value::as_str),
        Some("session")
    );
    assert!(lines[0].get("id").is_some());
    assert_eq!(
        lines[1].get("type").and_then(Value::as_str),
        Some("message_start")
    );
    assert_eq!(
        lines[1].pointer("/message/role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(lines[2], json!({"type":"agent_start"}));
    assert_eq!(lines[3], json!({"type":"turn_start"}));
    // The user message_start (index 1, pinned above), then the assistant
    // stream (start/update/end).
    let mi = 1usize;
    let mut a_start: Option<usize> = None;
    let mut delta: Option<usize> = None;
    let mut t_start: Option<usize> = None;
    let mut t_end: Option<usize> = None;
    for (idx, l) in lines.iter().enumerate() {
        let t = l.get("type").and_then(Value::as_str);
        let role = l.pointer("/message/role").and_then(Value::as_str);
        if a_start.is_none() && t == Some("message_start") && role == Some("assistant") {
            a_start = Some(idx);
        }
        if delta.is_none() && t == Some("message_update") {
            delta = Some(idx);
        }
        if t_start.is_none() && t == Some("tool_execution_start") {
            t_start = Some(idx);
        }
        if t_end.is_none() && t == Some("tool_execution_end") {
            t_end = Some(idx);
        }
    }
    assert!(
        mi < a_start.expect("assistant message_start"),
        "order: user start before assistant start"
    );
    assert!(
        a_start < Some(delta.expect("text_delta update")),
        "order: start before deltas"
    );
    // The final assistant message_end (the text turn) follows its delta.
    let d = delta.unwrap();
    assert!(
        lines
            .iter()
            .skip(d + 1)
            .any(
                |l| l.get("type").and_then(Value::as_str) == Some("message_end")
                    && l.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
            ),
        "order: deltas before end"
    );
    assert_eq!(
        lines[d]
            .pointer("/assistantMessageEvent/type")
            .and_then(Value::as_str),
        Some("text_delta")
    );
    assert_eq!(
        lines[d]
            .pointer("/assistantMessageEvent/delta")
            .and_then(Value::as_str),
        Some("all good")
    );
    let end = lines
        .iter()
        .rev()
        .find(|l| {
            l.get("type").and_then(Value::as_str) == Some("message_end")
                && l.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
        })
        .expect("final assistant end");
    assert_eq!(
        end.pointer("/message/stopReason").and_then(Value::as_str),
        Some("stop")
    );
    assert!(
        end.pointer("/message/content/0/text")
            .and_then(Value::as_str)
            == Some("all good")
    );
    // Tool execution pair with upstream field names.
    let ts = t_start.expect("tool_execution_start");
    let te = t_end.expect("tool_execution_end");
    assert!(
        a_start < Some(ts) && ts < te,
        "tool pair after assistant message"
    );
    assert_eq!(
        lines[ts].get("toolName").and_then(Value::as_str),
        Some("bash")
    );
    assert_eq!(
        lines[ts].pointer("/args/command").and_then(Value::as_str),
        Some("echo ok")
    );
    assert!(lines[ts].get("toolCallId").is_some());
    assert_eq!(lines[te].get("toolCallId"), lines[ts].get("toolCallId"));
    assert_eq!(lines[te].get("isError"), Some(&json!(false)));
    // The run ends with agent_end (messages + willRetry:false).
    let last = lines.last().expect("events");
    assert_eq!(last.get("type").and_then(Value::as_str), Some("agent_end"));
    assert_eq!(last.get("willRetry"), Some(&json!(false)));
    assert!(last
        .get("messages")
        .and_then(Value::as_array)
        .is_some_and(|m| !m.is_empty()));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-610: print mode error exit — a failing run exits 1 with the error on
/// stderr and empty stdout (upstream text-mode exit codes).
#[test]
fn wp610_print_mode_error_exit() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp610e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // The scripted model calls a tool that exists nowhere: the run fails
    // with a model-script error (upstream text mode: exit 1, stderr, no
    // stdout).
    let scenario = json!({
        "user_messages": ["hi"],
        "model_scripts": [[{ "tool_call": { "name": "nope", "input": {} } }]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--print")
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(1),
        "upstream error exit ({:?}) stdout: {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "no stdout on error"
    );
    assert!(stderr.contains("unknown tool"), "error on stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&tmp);
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Parse the NDJSON event log on a run's stdout; panics on a non-JSON line.
fn event_log(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
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
        .arg("--extension-dir")
        .arg(ext.join("hello.ts"))
        .arg("--extension-dir")
        .arg(ext.join("permission-gate.ts"))
        .arg("--extension-dir")
        .arg(ext.join("with-deps"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
        .collect();

    // Every stdout line is a machine-readable event with a `kind`.
    assert!(
        lines.iter().all(|l| l.get("kind").is_some()),
        "non-event stdout: {stdout}"
    );

    let tool_execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(tool_execs.len(), 3, "expected 3 tool executions\n{stdout}");

    let bash_ok = tool_execs
        .iter()
        .find(|t| {
            t["name"] == "bash"
                && t["blocked"] == json!(false)
                && t["input"]["command"] == "echo bridge-ok"
        })
        .expect("bash echo bridge-ok missing\n{stdout}");
    assert!(bash_ok["result_summary"]
        .as_str()
        .unwrap()
        .contains("bridge-ok"));

    let hello = tool_execs
        .iter()
        .find(|t| t["name"] == "hello")
        .expect("hello extension tool call missing\n{stdout}");
    assert!(hello["result_summary"]
        .as_str()
        .unwrap()
        .contains("Hello, bridge!"));

    let blocked = tool_execs
        .iter()
        .find(|t| t["blocked"] == json!(true))
        .expect("blocked bash missing\n{stdout}");
    // WP-505: the pi-rust core is headless (no UI methods advertised), so the
    // unmodified permission-gate takes the upstream no-UI default path.
    assert_eq!(blocked["input"]["command"], "sudo true");
    assert_eq!(
        blocked["result_summary"],
        "Blocked by extension: Dangerous command blocked (no UI for confirmation)"
    );

    // The tool_call decision for the blocked command carries the handler's block + reason.
    let decisions: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "decision").collect();
    let gate = decisions
        .iter()
        .find(|d| d["name"] == "tool_call" && d["request"]["input"]["command"] == "sudo true")
        .expect("sudo tool_call decision missing\n{stdout}");
    assert_eq!(gate["response"]["result"]["block"], json!(true));
    assert_eq!(
        gate["response"]["result"]["reason"],
        "Dangerous command blocked (no UI for confirmation)"
    );

    // Turn boundaries and model outputs (turn 0 = 3 tool calls, turn 1 = text).
    assert!(lines
        .iter()
        .any(|l| l["kind"] == "turn_start" && l["turnIndex"] == 0));
    assert!(lines
        .iter()
        .any(|l| l["kind"] == "turn_end" && l["turnIndex"] == 1));
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2);
    assert_eq!(
        model_outputs[0]["toolCalls"],
        json!(["bash", "hello", "bash"])
    );

    // Final transcript: full session = 1 user + 2 assistant + 3 tool results,
    // all as upstream v3 `message` entries.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries = last["session"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 6, "transcript entries wrong\n{stdout}");
    assert!(
        entries.iter().all(|e| e["type"] == "message"),
        "upstream v3 entry types: {entries:?}"
    );
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec![
            "user",
            "assistant",
            "toolResult",
            "toolResult",
            "toolResult",
            "assistant"
        ]
    );
    assert_eq!(last["session"]["messages"].as_array().unwrap().len(), 6);

    // The session JSONL file was written: v3 header + same 6 entries.
    let file_lines = std::fs::read_to_string(&session_file)
        .unwrap()
        .lines()
        .count();
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
        .arg("--extension-dir")
        .arg(ext.join("prompt-customizer.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The handler ran cleanly: no extension_error line.
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    // The core sends the BuildSystemPromptOptions subset with read selected.
    let bas = lines
        .iter()
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("before_agent_start")
        })
        .expect("before_agent_start decision missing\n{stdout}");
    let selected = bas["request"]["systemPromptOptions"]["selectedTools"]
        .as_array()
        .expect("request systemPromptOptions.selectedTools missing\n{bas}");
    assert!(
        selected.iter().any(|t| t == "read"),
        "core selectedTools must include read\n{bas}"
    );

    // The response carries the rendered prompt with the tool_guidance section.
    let sp = bas["response"]["systemPrompt"]
        .as_str()
        .expect("rendered systemPrompt missing from before_agent_start response\n{stdout}");
    assert!(
        sp.contains("Use `read` for file contents"),
        "tool_guidance not rendered\n{sp}"
    );

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
        .arg("--extension-dir")
        .arg(ext.join("plan-mode"))
        .arg("--command")
        .arg("plan")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

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
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("tool_call")
                && l["request"]["input"]["command"] == "rm -rf /tmp/planmode-x"
        })
        .expect("rm tool_call decision missing\n{stdout}");
    assert_eq!(
        gate["response"]["result"]["block"],
        json!(true),
        "rm not blocked\n{gate}"
    );
    let reason = gate["response"]["result"]["reason"]
        .as_str()
        .expect("block reason missing\n{gate}");
    assert!(
        reason.contains("Plan mode: command blocked"),
        "unexpected reason: {reason}"
    );

    // (c) the safe bash executes (not blocked).
    assert!(
        lines.iter().any(
            |l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
                && l["name"] == "bash"
                && l["input"]["command"] == "ls"
                && l["blocked"] == json!(false)
        ),
        "ls tool_execution missing or blocked\n{stdout}",
    );

    // (d) the plan-mode context message was injected for the run.
    let bas = lines
        .iter()
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("before_agent_start")
        })
        .expect("before_agent_start decision missing\n{stdout}");
    assert!(
        bas.to_string().contains("[PLAN MODE ACTIVE]"),
        "plan-mode context message missing\n{bas}",
    );

    // (e) the plan-mode state entry was persisted to the session file by the core.
    let session_text = std::fs::read_to_string(&session_file).expect("session file missing");
    assert!(
        session_text
            .lines()
            .any(|l| l.contains("\"customType\":\"plan-mode\"")),
        "plan-mode custom entry missing from session file\n{session_text}",
    );

    // (f) WP-505: headless = no UI context (upstream noOp defaults): the plan-mode
    // agent_end select produces no dialog event and no response round-trip.
    assert!(
        !lines.iter().any(|l| l.get("kind").and_then(Value::as_str)
            == Some("extension_ui_request")
            && l.get("method").and_then(Value::as_str) == Some("select")),
        "headless select must not open a dialog\n{stdout}"
    );

    // Healthy run: no handler errors.
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
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
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The nested bash crossed the bridge (tool_call_request round-trip) and executed.
    let nested = lines
        .iter()
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("tool_execution")
                && l["name"] == "bash"
                && l["input"]["command"] == "echo nested-ok"
                && l["blocked"] == json!(false)
        })
        .expect("nested bash tool_execution missing\n{stdout}");
    assert!(
        nested["result_summary"]
            .as_str()
            .expect("nested summary missing")
            .contains("nested-ok"),
        "{nested}"
    );

    // The outer extension tool surfaced the nested result text.
    let outer = lines
        .iter()
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("tool_execution") && l["name"] == "nest"
        })
        .expect("nest tool_execution missing\n{stdout}");
    assert!(
        outer["result_summary"]
            .as_str()
            .expect("outer summary missing")
            .contains("nested-ok"),
        "{outer}"
    );

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
    std::fs::write(
        proj.join(".claude/rules/testing.md"),
        "# Testing\nAlways run tests.\n",
    )
    .unwrap();

    let ext = repo().join("upstream/packages/coding-agent/examples/extensions");
    let scenario = json!({ "user_messages": ["hi"], "model_scripts": [[{ "text": "done" }]] });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    // claude-rules scans ctx.cwd/.claude/rules; the sidecar inherits the core's cwd.
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&proj)
        .arg("--extension-dir")
        .arg(ext.join("claude-rules.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    let bas = lines
        .iter()
        .find(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("before_agent_start")
        })
        .expect("before_agent_start decision missing\n{stdout}");
    let sp = bas["response"]["systemPrompt"]
        .as_str()
        .expect("rendered systemPrompt missing\n{bas}");
    assert!(
        sp.contains("## Project Rules"),
        "rules section missing\n{sp}"
    );
    assert!(
        sp.contains(".claude/rules/testing.md"),
        "rule file missing\n{sp}"
    );

    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
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
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(3),
        "expected degraded exit 3\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // Built-in bash executes fine after the crash (degraded semantics).
    assert!(
        lines.iter().any(
            |l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
                && l["name"] == "bash"
                && l["input"]["command"] == "echo post-crash"
                && l["blocked"] == json!(false)
        ),
        "post-crash bash tool_execution missing\n{stdout}",
    );

    // The run was marked degraded/transport-failed in the event log.
    assert!(
        lines
            .iter()
            .any(|l| l.to_string().contains("degraded") || l.to_string().contains("sidecar")),
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
        if line.trim().is_empty() {
            continue;
        }
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
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
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
                    if n > 4 && buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
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
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg("mock-model")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    let _ = server.join();

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
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
        if line.trim().is_empty() {
            continue;
        }
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
        let Ok((mut s, _)) = listener.accept() else {
            return;
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
        use std::io::Write;
        let _ = s.write_all(body.as_bytes());
    });

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("OPENAI_API_KEY", "e2e-key")
        .env("OPENAI_BASE_URL", &base)
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg("gpt-4o-mini")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(&session_file)
        .arg("--print-usage")
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    let _ = server.join();

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
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
        "costUsd: {}",
        usage["costUsd"]
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
        if line.trim().is_empty() {
            continue;
        }
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
            let Ok((mut s, _)) = listener.accept() else {
                break;
            };
            let body_c = body.clone();
            std::thread::spawn(move || {
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
               "openai":{"type":"api_key","key":"openai-key"}})
        .to_string(),
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
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setmodel.ts");
    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("GROQ_BASE_URL", &base_groq)
        .env("OPENAI_BASE_URL", &base_openai)
        .arg("--provider")
        .arg("groq")
        .arg("--model")
        .arg("llama-3.3-70b-versatile")
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--scenario")
        .arg(&scenario)
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
        .collect();
    let dump = lines.last().unwrap()["session"].to_string();
    // Run 1 (prompt "first") was served by the original groq driver.
    assert!(
        dump.contains("groq-text"),
        "first run lost its driver: {dump}"
    );
    // Run 2 (prompt "second") was served by the post-swap openai driver.
    assert!(dump.contains("second"), "second run missing: {dump}");
    assert!(
        dump.contains("openai-text"),
        "second run not served by openai: {dump}"
    );

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
    assert!(
        !file.contains("nope-model"),
        "declined model leaked: {file}"
    );
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
        .arg("--extension-dir")
        .arg(ext.join("hello.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    // The original upstream SessionManager opens and reads the file.
    let node = std::env::var("PI_RUST_NODE").unwrap_or_else(|_| "node".into());
    // Paths travel via env: a native Windows path is neither a valid ESM
    // specifier (file://D:\... is an invalid URL) nor a valid JS string
    // literal (unescaped backslashes) embedded in the script below.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("upstream/packages/coding-agent/dist/index.js")
        .to_string_lossy()
        .to_string();
    let script = r#"
import { pathToFileURL } from "node:url";
const { SessionManager } = await import(pathToFileURL(process.env.PI_RUST_UPSTREAM_DIST).href);
const sm = await SessionManager.open(process.env.PI_RUST_SESSION_FILE);
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
"#;
    let node_out = Command::new(&node)
        .arg("--input-type=module")
        .arg("--eval")
        .arg(script)
        .env("PI_RUST_UPSTREAM_DIST", dist)
        .env("PI_RUST_SESSION_FILE", &session_file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("running node");
    let node_stdout = String::from_utf8_lossy(&node_out.stdout).to_string();
    let node_stderr = String::from_utf8_lossy(&node_out.stderr).to_string();
    assert!(
        node_stdout.contains("UPSTREAM-READ-OK"),
        "upstream SessionManager failed to read pi-rust session\n{node_stdout}\n{node_stderr}"
    );
}

/// S5: --resume continues an existing session file (same id, chain grows).
#[test]
fn resume_appends_to_existing_session() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-resume-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent");
    let f = tmp.join("s.jsonl");
    let mk = |msgs: Vec<&str>| -> Value {
        json!({"user_messages": msgs, "model_scripts": msgs.iter().map(|_m| vec![json!({"text": "ok: "})]).collect::<Vec<_>>()})
    };
    let s1 = tmp.join("s1.json");
    std::fs::write(&s1, mk(vec!["first"]).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&s1)
        .arg("--session-file")
        .arg(&f)
        .output()
        .expect("run 1");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id1: Value =
        serde_json::from_str(std::fs::read_to_string(&f).unwrap().lines().next().unwrap()).unwrap();
    let lines1 = std::fs::read_to_string(&f).unwrap().lines().count();
    assert_eq!(lines1, 3, "header + user + assistant");

    let s2 = tmp.join("s2.json");
    std::fs::write(&s2, mk(vec!["second"]).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--resume")
        .arg(&f)
        .arg("--scenario")
        .arg(&s2)
        .output()
        .expect("run 2");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let raw = std::fs::read_to_string(&f).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(
        lines.len(),
        lines1 + 2,
        "resume appends, never rewrites: {lines:?}"
    );
    let h: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(h["id"], id1["id"], "resume keeps the session id");
    // Final transcript of run 2 contains BOTH messages.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let msgs = last["session"]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4, "transcript = 2 + 2 messages");
    let first_entry: Value = serde_json::from_str(lines[1]).unwrap();
    assert!(
        first_entry["parentId"].is_null(),
        "first entry has no parent"
    );
    let prev: Value = serde_json::from_str(lines[lines1 - 1]).unwrap();
    let appended: Value = serde_json::from_str(lines[lines1]).unwrap();
    assert_eq!(
        appended["parentId"], prev["id"],
        "chain continues from the old leaf"
    );
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
    std::fs::write(
        &s1,
        json!({"user_messages":["base"],"model_scripts":[[{"text":"base done"}]]}).to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&s1)
        .arg("--session-file")
        .arg(&f)
        .output()
        .expect("base run");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let base_raw = std::fs::read_to_string(&f).unwrap();
    let base_id: Value = serde_json::from_str(base_raw.lines().next().unwrap()).unwrap();

    let s2 = tmp.join("s2.json");
    std::fs::write(
        &s2,
        json!({"user_messages":["branch"],"model_scripts":[[{"text":"branch done"}]]}).to_string(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--fork")
        .arg(&f)
        .arg("--scenario")
        .arg(&s2)
        .output()
        .expect("fork run");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    // Original untouched.
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        base_raw,
        "fork must not modify the original"
    );
    // Final transcript names the new file + parent id.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_ne!(last["session"]["id"], base_id["id"], "fork gets a new id");
    assert_eq!(
        last["session"]["parentSession"], base_id["id"],
        "fork records parentSession"
    );
    let new_file = PathBuf::from(last["session"]["file"].as_str().unwrap());
    assert!(new_file.exists(), "fork file exists: {new_file:?}");
    let nh: Value = serde_json::from_str(
        std::fs::read_to_string(&new_file)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(nh["parentSession"], base_id["id"]);
}

/// S5: --list-sessions lists agent-dir sessions as NDJSON (no sidecar).
#[test]
fn list_sessions_lists_agent_sessions() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-list-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent");
    let s1 = tmp.join("s1.json");
    std::fs::write(
        &s1,
        json!({"user_messages":["one"],"model_scripts":[[{"text":"a"}]]}).to_string(),
    )
    .unwrap();
    for _i in 1..=2 {
        let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
            .arg("--agent-dir")
            .arg(&agent_dir)
            .arg("--scenario")
            .arg(&s1)
            .output()
            .expect("run");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--list-sessions")
        .output()
        .expect("list");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "two sessions listed: {stdout}");
    for v in &lines {
        assert!(
            v.get("id").and_then(Value::as_str).is_some(),
            "entry has id: {v}"
        );
        assert!(
            v.get("file").and_then(Value::as_str).is_some(),
            "entry has file: {v}"
        );
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
    std::fs::write(
        &s1,
        json!({"user_messages":["a"],"model_scripts":[[{"text":"x"}]]}).to_string(),
    )
    .unwrap();
    let set_var = |c: &mut std::process::Command| {
        c.env("PI_CODING_AGENT_DIR", &env_agent);
    };

    // 1) env var default: no --agent-dir, PI_CODING_AGENT_DIR set.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    set_var(&mut cmd);
    let out = cmd
        .arg("--scenario")
        .arg(&s1)
        .output()
        .expect("run with env agent dir");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let file = last["session"]["file"].as_str().unwrap().to_string();
    assert!(
        file.starts_with(env_agent.to_string_lossy().as_ref()),
        "session under env agent dir: {file}"
    );

    // 2) --isolated: session file NOT under the env agent dir.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    set_var(&mut cmd);
    let out = cmd
        .arg("--isolated")
        .arg("--scenario")
        .arg(&s1)
        .output()
        .expect("run isolated");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    let file = last["session"]["file"].as_str().unwrap().to_string();
    assert!(
        !file.starts_with(env_agent.to_string_lossy().as_ref()),
        "isolated run must not use the env agent dir: {file}"
    );
    assert!(
        file.starts_with(std::env::temp_dir().to_string_lossy().as_ref()),
        "isolated run uses a temp dir: {file}"
    );
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
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent)
        .arg("--template")
        .arg("greet")
        .arg("World")
        .output()
        .expect("run with template");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let user: Vec<Value> = lines
        .iter()
        .filter(|e| e["kind"] == "user_input")
        .cloned()
        .collect();
    assert_eq!(user.len(), 1, "one user message: {lines:?}");
    assert_eq!(user[0]["text"], "Greet World nicely, in World style");
}

/// S7: unknown template is a typed error, non-zero exit.
#[test]
fn template_unknown_is_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-tpl-err-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&tmp)
        .arg("--template")
        .arg("missing")
        .output()
        .expect("run with unknown template");
    assert_ne!(out.status.code(), Some(0), "unknown template must fail");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("missing"), "{stderr}");
}

/// WP-408: --image <path> attaches a base64 image part to the prompt user
/// message; the OpenAI wire carries a data-URL image_url part and the
/// session file keeps the pi-ai image block.
#[test]
fn provider_e2e_image_attachment_openai() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m4-image-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Minimal PNG: 8-byte magic + a few payload bytes.
    let png: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x01, 0x02, 0x03, 0x04,
    ];
    let image_path = tmp.join("pic.png");
    std::fs::write(&image_path, png).unwrap();

    const B64_TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut expected_b64 = String::new();
    for chunk in png.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        expected_b64.push(B64_TABLE[(b0 >> 2) as usize] as char);
        expected_b64.push(B64_TABLE[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
        expected_b64.push(if chunk.len() > 1 {
            B64_TABLE[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        expected_b64.push(if chunk.len() > 2 {
            B64_TABLE[(b2 & 63) as usize] as char
        } else {
            '='
        });
    }

    let c1 =
        json!({"choices":[{"delta":{"role":"assistant","content":"I see a picture"}}]}).to_string();
    let sse = format!("data: {c1}\ndata: [DONE]\n");
    let mut resp = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
    for line in sse.split_inclusive('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let b = line.as_bytes();
        resp.push_str(&format!("{:x}\r\n", b.len()));
        resp.push_str(line);
        resp.push_str("\r\n");
    }
    resp.push_str("0\r\n\r\n");

    let capture = tmp.join("captured-request.txt");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let capture_in = capture.clone();
    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let mut buf = Vec::new();
        let mut tmp2 = [0u8; 8192];
        loop {
            match s.read(&mut tmp2) {
                Ok(0) | Err(_) => break,
                Ok(k) => {
                    buf.extend_from_slice(&tmp2[..k]);
                    // Complete request = header terminator + content-length body.
                    if let Some(h) = String::from_utf8_lossy(&buf).find("\r\n\r\n") {
                        let head = &buf[..h];
                        let len = head
                            .windows(15)
                            .find_map(|w| {
                                let s = String::from_utf8_lossy(w);
                                let lower = s.to_lowercase();
                                lower
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
            }
        }
        let _ = std::fs::write(&capture_in, &buf);
        let _ = s.write_all(resp.as_bytes());
    });

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("OPENAI_API_KEY", "e2e-key")
        .env("OPENAI_BASE_URL", &base)
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg("gpt-4o-mini")
        .arg("--prompt")
        .arg("describe the image")
        .arg("--image")
        .arg(&image_path)
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    let _ = server.join();

    let captured = std::fs::read_to_string(&capture).unwrap_or_default();
    let data_url = format!("data:image/png;base64,{expected_b64}");
    assert!(
        captured.contains(&data_url),
        "request missing data URL: {data_url}\ncaptured tail: {}",
        &captured[captured.len().saturating_sub(400)..]
    );

    let file = std::fs::read_to_string(&session_file).unwrap();
    let has_image_block = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .any(|e| {
            e.pointer("/message/content")
                .and_then(Value::as_array)
                .is_some_and(|c| {
                    c.iter()
                        .any(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                })
        });
    assert!(has_image_block, "session file has no image block:\n{file}");
}

/// WP-408: --image against a text-only registry model is a clear, typed
/// startup error (no model call).
/// WP-408: --image on a text-only model is a typed error.
///
/// WP-1403 (D14): the bundled catalog is generated from the live models.dev
/// API on every upstream build — `openai/gpt-4`, which this test used, was
/// dropped from the live catalog (and no text-only openai model remains), so
/// the test now uses a deterministic core-native models.json model instead
/// of a bundled one (same capability gate, no external drift).
#[test]
fn provider_e2e_image_model_not_capable() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m4-imagecap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let image_path = tmp.join("pic.png");
    std::fs::write(
        &image_path,
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
    )
    .unwrap();

    // Text-only model in a core-native models.json provider (the capability
    // gate fires before any API call, so baseUrl/apiKey are never used).
    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{
  "providers": {
    "mocknotimage": {
      "name": "Mock Not Image",
      "baseUrl": "http://127.0.0.1:1/v1",
      "api": "openai-completions",
      "apiKey": "e2e-key",
      "models": [{ "id": "texty-1", "name": "Texty One", "input": ["text"], "contextWindow": 4096, "maxTokens": 512 }]
    }
  }
}
"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("mocknotimage")
        .arg("--model")
        .arg("texty-1")
        .arg("--prompt")
        .arg("hi")
        .arg("--image")
        .arg(&image_path)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .output()
        .expect("running pi-rust");
    assert_ne!(
        out.status.code(),
        Some(0),
        "text-only model with --image must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(stderr.contains("does not accept image input"), "{stderr}");
    assert!(stderr.contains("texty-1"), "{stderr}");
}

/// WP-408: --image file errors are typed (missing file, not-an-image).
#[test]
fn provider_e2e_image_file_errors_typed() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-m4-imageerr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("fake")
        .arg("--prompt")
        .arg("hi")
        .arg("--image")
        .arg(tmp.join("missing.png"))
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_ne!(out.status.code(), Some(0));
    assert!(stderr.contains("cannot read file"), "{stderr}");

    let txt = tmp.join("not-image.png");
    std::fs::write(&txt, b"this is text, not an image").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("fake")
        .arg("--prompt")
        .arg("hi")
        .arg("--image")
        .arg(&txt)
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_ne!(out.status.code(), Some(0));
    assert!(stderr.contains("not a supported image format"), "{stderr}");
}

/// WP-506: an extension tool named `read` shadows the built-in read — execution
/// reaches the extension (upstream registry precedence), pinned by the fixture's
/// result text. Test-only fixture: tests/fixtures/shadow-read.ts.
#[test]
fn extension_tool_shadows_builtin() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp506-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let scenario = json!({
        "user_messages": ["read x"],
        "model_scripts": [[
            { "tool_call": { "name": "read", "input": { "path": "x" } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/shadow-read.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(execs.len(), 1, "expected one tool execution\n{stdout}");
    assert_eq!(execs[0]["name"], "read");
    assert_eq!(execs[0]["blocked"], json!(false));
    // The fixture's execute() ran, not the built-in read (which would error on
    // the missing file `x`).
    assert_eq!(execs[0]["result_summary"], "shadow-read:x");
}

// --- M7/WP-701: extension-registered providers (registerProvider) ---

/// S3: a fixture extension registering a fake provider drives a full
/// conversation (text + tool call) end-to-end: turn 1 = text + bash tool
/// call, the core executes it, turn 2 = final text. Exit 0.
#[test]
fn wp701_extension_provider_full_conversation() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp701-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/fake-provider.ts");
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("faketest")
        .arg("--model")
        .arg("faketest/echo-1")
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--prompt")
        .arg("run the check")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // No handler errors.
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    // The fake provider's tool call executed (built-in bash round trip).
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(execs.len(), 1, "expected one tool execution\n{stdout}");
    assert_eq!(execs[0]["name"], "bash");
    assert_eq!(execs[0]["input"]["command"], "echo fake-provider-ok");
    assert_eq!(execs[0]["blocked"], json!(false));
    assert!(execs[0]["result_summary"]
        .as_str()
        .unwrap_or("")
        .contains("fake-provider-ok"));

    // Turn 0: text + the tool call; turn 1: the final text.
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2, "expected 2 model outputs\n{stdout}");
    assert_eq!(model_outputs[0]["toolCalls"], json!(["bash"]));
    assert_eq!(model_outputs[0]["text"], "checking with bash");
    assert_eq!(model_outputs[1]["text"], "fake provider done");
    assert_eq!(model_outputs[1]["toolCalls"], json!([]));

    // Final transcript: user + assistant(tool call) + toolResult + assistant(text).
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "toolResult", "assistant"]);
    // Assistant messages carry the extension model's real labels.
    let assistants = entries
        .iter()
        .filter(|e| e["message"]["role"] == "assistant");
    for a in assistants {
        assert_eq!(a["message"]["provider"], "faketest");
        assert_eq!(a["message"]["model"], "faketest/echo-1");
    }
    assert_eq!(
        entries.last().unwrap()["message"]["content"][0]["text"],
        "fake provider done"
    );

    // Provider usage was recorded in the session file (from the done messages).
    let file = std::fs::read_to_string(&session_file).unwrap();
    let usage_entries: Vec<&str> = file
        .lines()
        .filter(|l| l.contains("\"type\":\"usage\""))
        .collect();
    assert_eq!(usage_entries.len(), 2, "usage entries: {file}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// S3: a fixture variant whose streamSimple pushes a terminal error event ->
/// the run surfaces a model error (exit 2 in NDJSON mode, message on stderr).
#[test]
fn wp701_extension_provider_error_surfaces_as_model_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp701e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/fake-provider-error.ts");
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("faketest-error")
        .arg("--model")
        .arg("faketest-error/boom-1")
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(2),
        "model error run exits 2\n{stderr}"
    );
    assert!(
        stderr.contains("boom from fake provider"),
        "provider errorMessage must surface on stderr: {stderr}",
    );

    // The run failed before any tool call; the transcript holds the user
    // message only.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let msgs = last["session"]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1, "only the user message: {last}");
    assert_eq!(msgs[0]["role"], "user");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// S3: a bundled-registry model still works with a provider extension loaded
/// (fallback unchanged: the extension's model list does not shadow the
/// bundled driver).
#[test]
fn wp701_bundled_model_fallback_with_extension_loaded() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp701f-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let base = sse_loop_mock("mock-openai-text");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("OPENAI_API_KEY", "e2e-key")
        .env("OPENAI_BASE_URL", &base)
        .arg("--provider")
        .arg("openai")
        .arg("--model")
        .arg("gpt-4o-mini")
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/fake-provider.ts"))
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let last = lines.last().expect("no stdout lines");
    let dump = last["session"].to_string();
    assert!(
        dump.contains("mock-openai-text"),
        "bundled driver must serve the run: {dump}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// S3: set_model (sidecar->core) accepts an extension-registered id — the
/// fixture tool switches the active model mid-run (a scripted fake turn), the
/// core validates it against the extension list, and the NEXT turn is served
/// by the extension provider over the bridge. One model_change entry.
#[test]
fn wp701_set_model_accepts_extension_id() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp701s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/fake-provider.ts");
    let scenario = json!({
        "user_messages": ["switch"],
        "model_scripts": [[
            { "tool_call": { "name": "switch_to_fake", "input": {} } },
            { "text": "should never be used" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--scenario")
        .arg(&scen)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The setModel round-trip was accepted (ok:true in the tool result).
    let exec = lines
        .iter()
        .find(|l| l["kind"] == "tool_execution" && l["name"] == "switch_to_fake")
        .expect("switch_to_fake tool_execution missing\n{stdout}");
    assert_eq!(exec["result_summary"], "switched:true");

    // Turn 2 was served by the extension provider, not the scripted fake.
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2, "expected 2 model outputs\n{stdout}");
    assert_eq!(model_outputs[1]["text"], "fake provider done");

    let last = lines.last().expect("no stdout lines");
    let dump = last["session"].to_string();
    assert!(
        !dump.contains("should never be used"),
        "scripted turn leaked: {dump}"
    );

    // Exactly one model_change entry: the accepted extension switch.
    let file = std::fs::read_to_string(&session_file).unwrap();
    let changes: Vec<Value> = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("model_change"))
        .collect();
    assert_eq!(changes.len(), 1, "model_change entries: {file}");
    assert_eq!(changes[0]["provider"], "faketest");
    assert_eq!(changes[0]["modelId"], "faketest/echo-1");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// S3: a provider name that is neither bundled nor extension-registered is a
/// typed startup error (no silent fake-driver fallback).
#[test]
fn wp701_unknown_extension_provider_is_typed_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp701u-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("not-registered-anywhere")
        .arg("--model")
        .arg("not-registered-anywhere/m-1")
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/fake-provider.ts"))
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(tmp.join("agent"))
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .output()
        .expect("running pi-rust");
    assert_ne!(out.status.code(), Some(0), "unknown provider must fail");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains("not-registered-anywhere/m-1"),
        "typed error names the model: {stderr}"
    );
    assert!(
        stderr.contains("extension"),
        "typed error mentions the extension registry: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// --- M7/WP-702: MCP transport (stdio) ---

/// S4: an agent-dir mcp.json declares the fixture server; the run sees the
/// `mcp__<server>__<tool>` tools (registered post-ready via `tools_changed`)
/// and both calls round-trip bridge → fixture process → bridge with the
/// correct results. Exit 0.
#[test]
fn wp702_mcp_config_stdio_round_trip() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp702a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let fixture = repo().join("tests/fixtures/mcp-server-fixture.mjs");
    // json! escapes the path properly (raw Windows paths are invalid JSON).
    let mcp = json!({
        "mcpServers": {
            "fixture": {
                "command": "node",
                "args": [fixture.to_string_lossy(), "--spawn-log", tmp.join("spawns.log").to_string_lossy()],
                "exposure": "direct"
            }
        }
    });
    std::fs::write(agent_dir.join("mcp.json"), mcp.to_string()).unwrap();
    let scenario = json!({
        "user_messages": ["use the mcp tools"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__fixture__echo", "input": { "text": "bridge-mcp" } } },
            { "tool_call": { "name": "mcp__fixture__counter", "input": {} } },
            { "text": "mcp done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // No handler errors.
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    // Both MCP tools executed with the fixture's results.
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(execs.len(), 2, "expected 2 tool executions\n{stdout}");
    let echo = execs
        .iter()
        .find(|t| t["name"] == "mcp__fixture__echo")
        .expect("echo call missing\n{stdout}");
    assert_eq!(echo["blocked"], json!(false));
    assert_eq!(
        echo["result_summary"], "echo:bridge-mcp",
        "echo round-trip: {echo}"
    );
    let counter = execs
        .iter()
        .find(|t| t["name"] == "mcp__fixture__counter")
        .expect("counter call missing\n{stdout}");
    assert_eq!(
        counter["result_summary"], "count:1",
        "counter round-trip: {counter}"
    );

    // Turn 0: the two MCP tool calls; turn 1: the final text.
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2, "expected 2 model outputs\n{stdout}");
    assert_eq!(
        model_outputs[0]["toolCalls"],
        json!(["mcp__fixture__echo", "mcp__fixture__counter"])
    );
    assert_eq!(model_outputs[1]["text"], "mcp done");

    // Final transcript: user + assistant(tool calls) + 2 toolResults + assistant.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "toolResult", "toolResult", "assistant"]
    );
    for r in entries
        .iter()
        .filter(|e| e["message"]["role"] == "toolResult")
    {
        assert_eq!(r["message"]["isError"], json!(false));
    }

    // E3S3: the core owns the connection — exactly ONE fixture process was
    // spawned, so a live sidecar did not double-connect the config server.
    let spawns = std::fs::read_to_string(tmp.join("spawns.log")).expect("spawn log missing");
    assert_eq!(
        spawns.lines().count(),
        1,
        "the configured server must be connected exactly once (no double-connect): {spawns}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// E3S5/WP-1405 (D13 final form): the sidecar's built-in mcp extension is
/// gone, so an extension's `pi.registerMcpServer` call is still accepted by
/// the unmodified upstream runner (the API is part of the mirror, ADR 0002)
/// but the server is NEVER connected — no `mcp__fixture_reg__*` tool is ever
/// declared and the fixture process is never spawned. Pinning the E3S5
/// deletion (the WP-702 registration path "stayed until E3S5" — D13): with
/// the old sidecar this test is RED (the tool is declared and the server is
/// spawned on session_start).
#[test]
fn wp1405_register_mcp_server_not_hosted_pin() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1405r-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let spawn_log = tmp.join("spawns.log");
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/mcp-register-fixture.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--prompt")
        .arg("hi")
        .arg("--session-file")
        .arg(&session_file)
        .env("MCP_REGISTER_SPAWN_LOG", &spawn_log)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    // The extension itself loads (the API call is accepted by the unmodified
    // runner): 4 = fixture + tool-search + llama + codemode builtins (the
    // built-in mcp extension is gone — E3S5).
    assert!(
        stderr.contains("ready: 4 extensions"),
        "the register fixture must load (positive control)\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );
    // Pin 1: the registered server's tools are never declared to the model.
    let selected: Vec<&str> = lines
        .iter()
        .filter(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("before_agent_start")
        })
        .filter_map(|l| l["request"]["systemPromptOptions"]["selectedTools"].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        !selected.iter().any(|t| t.starts_with("mcp__fixture_reg__")),
        "the registered server's tools must never be declared (no host)\n{selected:?}"
    );
    // Pin 2: no execution of a registered-server tool.
    assert!(
        !lines.iter().any(
            |l| l.get("kind").and_then(Value::as_str) == Some("tool_execution")
                && l["name"]
                    .as_str()
                    .is_some_and(|n| n.starts_with("mcp__fixture_reg__"))
        ),
        "no mcp__fixture_reg__ tool may execute\n{stdout}"
    );
    // Pin 3: the server process was never spawned.
    assert!(
        !spawn_log.exists(),
        "the registered server must never be spawned (no sidecar host)\n{}",
        std::fs::read_to_string(&spawn_log).unwrap_or_default()
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// S4: crash/restart semantics per upstream — the fixture server exits after
/// the first successful call; there is NO auto-restart, the next call
/// reconnects lazily and gets a fresh process (counter reset to 1, not 2).
#[test]
fn wp702_mcp_crash_lazy_reconnect() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp702c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let fixture = repo().join("tests/fixtures/mcp-server-fixture.mjs");
    let mcp = json!({
        "mcpServers": {
            "fixture": {
                "command": "node",
                "args": [fixture.to_string_lossy(), "--die-after-first-call", "--spawn-log", tmp.join("spawns.log").to_string_lossy()],
                "exposure": "direct"
            }
        }
    });
    std::fs::write(agent_dir.join("mcp.json"), mcp.to_string()).unwrap();
    let scenario = json!({
        "user_messages": ["count, let it die, count again"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__fixture__counter", "input": {} } },
            { "tool_call": { "name": "bash", "input": { "command": "sleep 0.3" } } },
            { "tool_call": { "name": "mcp__fixture__counter", "input": {} } },
            { "text": "reconnect ok" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("extension_error")),
        "unexpected extension_error\n{stdout}",
    );

    // The sleep between calls gives the client time to observe the server exit
    // (no auto-restart: the reconnect happens on the NEXT call).
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(execs.len(), 3, "expected 3 tool executions\n{stdout}");
    let counters: Vec<&Value> = execs
        .iter()
        .copied()
        .filter(|t| t["name"] == "mcp__fixture__counter")
        .collect();
    assert_eq!(counters.len(), 2, "expected 2 counter calls\n{stdout}");
    assert_eq!(
        counters[0]["result_summary"], "count:1",
        "first call: {}",
        counters[0]
    );
    // The second call runs against a FRESH process (lazy reconnect): the
    // counter restarted at 1.
    assert_eq!(
        counters[1]["result_summary"], "count:1",
        "lazy reconnect must get a fresh process: {}",
        counters[1]
    );
    let sleep_exec = execs
        .iter()
        .find(|t| t["name"] == "bash")
        .expect("bash sleep missing\n{stdout}");
    assert_eq!(sleep_exec["blocked"], json!(false));

    let last = lines.last().expect("no stdout lines");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    assert_eq!(
        entries.last().unwrap()["message"]["content"][0]["text"],
        "reconnect ok"
    );
    // E3S3: the reconnect is lazy + a FRESH process — exactly two spawns total
    // (initial connect + the one after the crash; no auto-restart, no double).
    let spawns = std::fs::read_to_string(tmp.join("spawns.log")).expect("spawn log missing");
    assert_eq!(
        spawns.lines().count(),
        2,
        "lazy reconnect must spawn one fresh process: {spawns}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// S4: a dead/unreachable server (bad command) never connected, so it offers
/// no tools — a scripted call to its (unregistered) tool name produces a tool
/// ERROR RESULT, not a core crash or a model-script abort. The run finishes.
#[test]
fn wp702_mcp_dead_server_tool_error_not_crash() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp702d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("mcp.json"),
        r#"{"mcpServers":{"bad":{"command":"definitely-not-a-command-wp702"}}}"#,
    )
    .unwrap();
    let scenario = json!({
        "user_messages": ["try the broken server"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__bad__echo", "input": { "text": "x" } } },
            { "text": "survived" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // The run completes (exit 0): the tool call is an error result, not a crash.
    assert_eq!(
        out.status.code(),
        Some(0),
        "dead server must not abort the run\n{stderr}\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The call executed as an error result with the core's typed message
    // (E3S3: the core owns the configured server, so the connect failure is
    // the core's, not the sidecar's "Unknown extension tool").
    let exec = lines
        .iter()
        .find(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__bad__echo")
        .expect("mcp__bad__echo execution missing\n{stdout}");
    assert_eq!(exec["blocked"], json!(false));
    assert!(
        exec["result_summary"]
            .as_str()
            .unwrap_or("")
            .contains("failed to connect"),
        "error result must carry the core's typed connect message: {exec}",
    );

    // The recorded toolResult is isError, and the run reached its final text.
    let last = lines.last().expect("no stdout lines");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "toolResult", "assistant"]);
    let tool_result = entries
        .iter()
        .find(|e| e["message"]["role"] == "toolResult")
        .unwrap();
    assert_eq!(
        tool_result["message"]["isError"],
        json!(true),
        "tool result must be an error: {tool_result}"
    );
    assert_eq!(
        entries.last().unwrap()["message"]["content"][0]["text"],
        "survived"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// S4/WP-703: a config-declared HTTP MCP server - the sidecar's upstream
/// streamable-HTTP transport does initialize + tool calls against the local
/// mock, assembles the SSE stream response, and the auth header (from the
/// WP703_SECRET env expansion) reaches the server while the secret value
/// stays out of every log line.
#[test]
fn wp703_mcp_http_round_trip() {
    let secret = "wp703-secret-token-123";
    // Mock server on an ephemeral 127.0.0.1 port; it reports the port on stdout.
    let mut child = Command::new("node")
        .arg(repo().join("tests/fixtures/mcp-http-fixture.mjs"))
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning mock MCP HTTP server");
    let mut port_reader = std::io::BufReader::new(child.stdout.take().expect("mock server stdout"));
    let mut port_line = String::new();
    port_reader
        .read_line(&mut port_line)
        .expect("mock server port line");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("mock server port");

    let tmp = std::env::temp_dir().join(format!("pi-rust-wp703-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("mcp.json"),
        format!(
            r#"{{"mcpServers":{{"httptest":{{"url":"http://127.0.0.1:{port}/m","exposure":"direct","headers":{{"Authorization":"${{WP703_SECRET}}"}}}}}}}}"#
        ),
    )
    .unwrap();
    let scenario = json!({
        "user_messages": ["call the http server"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__httptest__echo", "input": { "text": "ping" } } },
            { "tool_call": { "name": "mcp__httptest__echo", "input": { "text": "sse" } } },
            { "tool_call": { "name": "mcp__httptest__counter", "input": {} } },
            { "text": "http mcp done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .env("WP703_SECRET", secret)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}\n{stdout}"
    );

    // The secret never reaches any log line (sidecar stderr is inherited here).
    assert!(
        !stderr.contains(secret),
        "secret leaked into stderr:\n{stderr}"
    );

    let lines = event_log(&stdout);
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__httptest__echo")
        .collect();
    assert_eq!(execs.len(), 2, "both echo calls executed\n{stdout}");
    // Plain JSON response path: the server saw the expanded auth header.
    let first = execs[0]["result_summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        first.contains("echo:ping auth:"),
        "first echo failed: {first}"
    );
    assert!(
        first.contains(secret),
        "auth header must reach the server: {first}"
    );
    // SSE stream response path: assembled from the event stream by the transport.
    let second = execs[1]["result_summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        second.contains("echo:sse auth:"),
        "SSE stream response must assemble: {second}"
    );
    assert!(
        second.contains(secret),
        "auth header on the SSE path: {second}"
    );
    let counters: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__httptest__counter")
        .collect();
    assert_eq!(counters.len(), 1, "counter call executed\n{stdout}");
    // Server-side state survived the calls (one live connection, not per-call).
    let count_summary = counters[0]["result_summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert_eq!(
        count_summary, "count:1",
        "server-side counter state: {count_summary}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ============================================================================
// E3S3/WP-1403: core-native MCP hosting — the core connects and drives the
// configured MCP servers itself; since E3S5/WP-1405 the sidecar hosts NO MCP
// servers at all (its built-in mcp extension + the mcpConfig registration
// path are removed; `pi.registerMcpServer` is accepted by the unmodified
// runner but never connected — the negative pin above, D13). The
// sidecar-degraded runs below are the HARD acceptance: the existing MCP
// fixture scenarios (stdio + HTTP, exposure rules) pass with no sidecar at
// all.
// ============================================================================

/// HARD: the WP-702 stdio scenario with the **sidecar degraded** (spawn
/// failure) — the core connects the fixture itself, declares the
/// `direct`-exposure tools, executes both calls natively (no bridge hop),
/// and the results land in the transcript. Exit 0.
#[test]
fn wp1403_mcp_stdio_sidecar_degraded() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1403a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let fixture = repo().join("tests/fixtures/mcp-server-fixture.mjs");
    let mcp = json!({
        "mcpServers": {
            "fixture": {
                "command": "node",
                "args": [fixture.to_string_lossy()],
                "exposure": "direct"
            }
        }
    });
    std::fs::write(agent_dir.join("mcp.json"), mcp.to_string()).unwrap();
    let scenario = json!({
        "user_messages": ["use the mcp tools"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__fixture__echo", "input": { "text": "core-mcp" } } },
            { "tool_call": { "name": "mcp__fixture__counter", "input": {} } },
            { "text": "mcp done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1403-node")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    // The run must be the degraded one (no sidecar at all).
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    // Both MCP tools executed natively with the fixture's results.
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    assert_eq!(execs.len(), 2, "expected 2 tool executions\n{stdout}");
    let echo = execs
        .iter()
        .find(|t| t["name"] == "mcp__fixture__echo")
        .expect("echo call missing\n{stdout}");
    assert_eq!(echo["blocked"], json!(false));
    assert_eq!(
        echo["result_summary"], "echo:core-mcp",
        "core-native echo round-trip: {echo}"
    );
    let counter = execs
        .iter()
        .find(|t| t["name"] == "mcp__fixture__counter")
        .expect("counter call missing\n{stdout}");
    assert_eq!(
        counter["result_summary"], "count:1",
        "core-native counter round-trip: {counter}"
    );

    // Transcript: user + assistant(tool calls) + 2 toolResults + assistant.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "toolResult", "toolResult", "assistant"]
    );
    for r in entries
        .iter()
        .filter(|e| e["message"]["role"] == "toolResult")
    {
        assert_eq!(r["message"]["isError"], json!(false));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// HARD: the WP-703 HTTP scenario with the **sidecar degraded** — the core
/// speaks streamable HTTP (JSON + SSE response assembly) to the local mock
/// itself; the env-expanded auth header reaches the server. Exit 0.
#[test]
fn wp1403_mcp_http_sidecar_degraded() {
    let secret = "wp1403-secret-token-456";
    let mut child = Command::new("node")
        .arg(repo().join("tests/fixtures/mcp-http-fixture.mjs"))
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning mock MCP HTTP server");
    let mut port_reader = std::io::BufReader::new(child.stdout.take().expect("mock server stdout"));
    let mut port_line = String::new();
    port_reader
        .read_line(&mut port_line)
        .expect("mock server port line");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("mock server port");

    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1403b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("mcp.json"),
        format!(
            r#"{{"mcpServers":{{"httptest":{{"url":"http://127.0.0.1:{port}/m","exposure":"direct","headers":{{"Authorization":"${{WP1403_SECRET}}"}}}}}}}}"#
        ),
    )
    .unwrap();
    let scenario = json!({
        "user_messages": ["call the http server"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__httptest__echo", "input": { "text": "ping" } } },
            { "tool_call": { "name": "mcp__httptest__echo", "input": { "text": "sse" } } },
            { "tool_call": { "name": "mcp__httptest__counter", "input": {} } },
            { "text": "http mcp done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1403-node")
        .env("WP1403_SECRET", secret)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}\n{stdout}"
    );
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path\n{stderr}"
    );

    let lines = event_log(&stdout);
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__httptest__echo")
        .collect();
    assert_eq!(execs.len(), 2, "both echo calls executed\n{stdout}");
    let first = execs[0]["result_summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        first.contains("echo:ping auth:"),
        "first echo failed: {first}"
    );
    assert!(
        first.contains(secret),
        "auth header must reach the server: {first}"
    );
    let second = execs[1]["result_summary"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        second.contains("echo:sse auth:"),
        "SSE stream response must assemble: {second}"
    );
    let counters: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__httptest__counter")
        .collect();
    assert_eq!(counters.len(), 1, "counter call executed\n{stdout}");
    assert_eq!(
        counters[0]["result_summary"], "count:1",
        "server-side counter state (one live core connection): {}",
        counters[0]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// E3S3 exposure rules (upstream `getMcpToolExposure`) with the sidecar
/// alive: the probe extension records `before_agent_start.selectedTools`
/// (what the model is declared) and the core's `mcp_servers_change`
/// forward. Server default exposure `codemode` keeps tools out of the
/// model's set; the `toolExposure` override puts `echo` in (`direct`).
/// Both tools stay callable by explicit name (v0 core reachability), and
/// they execute natively (no `extension_tool_call` hop for config servers).
#[test]
fn wp1403_mcp_exposure_rules_and_change_event() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1403c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let fixture = repo().join("tests/fixtures/mcp-server-fixture.mjs");
    let mcp = json!({
        "mcpServers": {
            "cfg": {
                "command": "node",
                "args": [fixture.to_string_lossy()],
                "toolExposure": { "echo": "direct" }
            }
        }
    });
    std::fs::write(agent_dir.join("mcp.json"), mcp.to_string()).unwrap();
    let scenario = json!({
        "user_messages": ["call both"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__cfg__echo", "input": { "text": "declared" } } },
            { "tool_call": { "name": "mcp__cfg__counter", "input": {} } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/mcp-tools-probe.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The probe recorded the events as custom session entries (the core
    // persists pi.appendEntry calls).
    let file = std::fs::read_to_string(&session_file).expect("session file missing");
    let entries: Vec<Value> = file
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("session line not JSON: {l}: {e}"))
        })
        .collect();

    // The declared tool set: echo in (direct override), counter out
    // (server default codemode).
    let selected = entries
        .iter()
        .find(|e| e["type"] == "custom" && e["customType"] == "wp1403-selected-tools")
        .expect("selectedTools probe entry missing\n{file}");
    let tools: Vec<String> = selected["data"]["selectedTools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    assert!(
        tools.iter().any(|t| t == "mcp__cfg__echo"),
        "direct override must be declared to the model: {tools:?}"
    );
    assert!(
        !tools.iter().any(|t| t == "mcp__cfg__counter"),
        "codemode-default tool must NOT be declared: {tools:?}"
    );

    // The core forwarded its configured server as mcp_servers_change.
    let change = entries
        .iter()
        .find(|e| e["type"] == "custom" && e["customType"] == "wp1403-mcp-change")
        .expect("mcp_servers_change probe entry missing\n{file}");
    let servers: Vec<String> = change["data"]["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    assert!(
        servers.iter().any(|s| s == "cfg"),
        "change carries the configured server: {servers:?}"
    );

    // Both tools executed natively (the sidecar never holds the config
    // server, so a bridge hop would have failed).
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution")
        .collect();
    let echo = execs
        .iter()
        .find(|t| t["name"] == "mcp__cfg__echo")
        .expect("echo execution missing\n{stdout}");
    assert_eq!(echo["result_summary"], "echo:declared", "{echo}");
    let counter = execs
        .iter()
        .find(|t| t["name"] == "mcp__cfg__counter")
        .expect("counter execution missing\n{stdout}");
    assert_eq!(counter["result_summary"], "count:1", "{counter}");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ============================================================================
// WP-704: compaction (auto trigger + summarization + events + session
// integration). Protocol v3; upstream core/compaction port (pin 64a15f4).
// ============================================================================

/// WP-704 e2e scenario: run 1 completes a short turn; run 2 sends a long
/// message that crosses the scenario `compact` threshold (window 1500,
/// reserve 400, keep 0 — the fake model has no catalog contextWindow, so the
/// override drives the auto trigger). Returns (scenario, filler text).
fn wp704_compaction_scenario() -> (Value, String) {
    let filler: String = std::iter::repeat_n("WP704LONG filler. ", 500).collect(); // ~4500 tokens
    let scenario = json!({
        "user_messages": ["short first turn", &filler],
        "model_scripts": [
            [{ "text": "first turn done" }],
            [
                { "tool_call": { "name": "bash", "input": { "command": "echo after-compaction" } } },
                { "text": "second run done" }
            ]
        ],
        "compact": { "window": 1500, "reserve": 400, "keep": 0 },
        "compact_summary": "WP704 SUMMARY"
    });
    (scenario, filler)
}

/// WP-704a/b: a scripted over-threshold session auto-compacts before the
/// run-2 model call: upstream-shaped `compaction` session entry (summary from
/// `compact_summary`, firstKeptEntryId = the long user entry, tokensBefore > 0,
/// systemMessage snapshot, no fromHook), a `compaction` NDJSON line, the
/// projected transcript (compactionSummary first; pre-compaction history
/// projected out, kept tail retained), exit 0.
#[test]
fn wp704_auto_compaction_threshold() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp704a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let (scenario, _filler) = wp704_compaction_scenario();
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The core emits one `compaction` event line (a kind line like tool_execution).
    let compact = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("compaction"))
        .expect("compaction NDJSON line missing\n{stdout}");
    assert_eq!(compact["summary"], "WP704 SUMMARY");
    assert_eq!(compact["reason"], "threshold");
    assert_eq!(compact["fromExtension"], json!(false));
    assert!(
        compact["tokensBefore"].as_u64().unwrap_or(0) > 0,
        "tokensBefore: {compact}"
    );
    let first_kept = compact["firstKeptEntryId"]
        .as_str()
        .expect("firstKeptEntryId")
        .to_string();

    // Session file: the upstream-shaped compaction entry.
    let file = std::fs::read_to_string(&session_file).expect("session file missing");
    let entries: Vec<Value> = file
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("session line not JSON: {l}: {e}"))
        })
        .collect();
    let comp = entries
        .iter()
        .find(|e| e["type"] == "compaction")
        .expect("compaction entry missing in session file\n{file}");
    assert_eq!(comp["summary"], "WP704 SUMMARY");
    assert_eq!(comp["firstKeptEntryId"], first_kept);
    assert!(
        comp["tokensBefore"].as_u64().unwrap_or(0) > 0,
        "entry tokensBefore: {comp}"
    );
    assert!(
        comp.get("fromHook").is_none(),
        "core compaction is not fromHook: {comp}"
    );
    assert!(
        comp.get("systemMessage").is_some(),
        "systemMessage snapshot: {comp}"
    );
    assert!(
        comp.get("parentId").is_some() && comp.get("timestamp").is_some(),
        "entry base fields: {comp}"
    );

    // firstKeptEntryId resolves to the long user message entry (the kept tail).
    let kept = entries
        .iter()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(first_kept.as_str()))
        .expect("firstKeptEntryId does not resolve to an entry");
    assert_eq!(kept["message"]["role"], "user");
    assert!(
        kept["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("WP704LONG"),
        "kept entry: {kept}"
    );

    // Transcript projection: the post-compaction model call sees the
    // compactionSummary + kept tail, without the pre-compaction history.
    let last = lines.last().expect("no transcript event");
    let msgs = last["session"]["messages"]
        .as_array()
        .expect("projected messages")
        .to_vec();
    assert_eq!(
        msgs[0]["role"], "compactionSummary",
        "projection starts with the summary: {msgs:?}"
    );
    assert_eq!(msgs[0]["content"][0]["text"], "WP704 SUMMARY");
    let all: String = msgs.iter().map(|m| m.to_string()).collect();
    assert!(
        !all.contains("first turn done"),
        "pre-compaction assistant text projected out: {all}"
    );
    assert!(
        !all.contains("short first turn"),
        "pre-compaction user text projected out: {all}"
    );
    assert!(
        all.contains("WP704LONG"),
        "kept user message stays in context: {all}"
    );
    assert!(
        all.contains("after-compaction"),
        "post-compaction turn kept: {all}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-704a: with a fixture extension loaded, the session_before_compact
/// decision (preparation + branchEntries) and the session_compact event
/// (saved entry) reach the sidecar — the fixture records both as custom
/// session entries.
#[test]
fn wp704_compaction_events_reach_extension() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp704b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let (scenario, _filler) = wp704_compaction_scenario();
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/compaction-events.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let comp = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("compaction"))
        .expect("compaction NDJSON line missing\n{stdout}");
    let first_kept = comp["firstKeptEntryId"]
        .as_str()
        .expect("firstKeptEntryId")
        .to_string();

    let file = std::fs::read_to_string(&session_file).expect("session file missing");
    let entries: Vec<Value> = file
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("session line not JSON: {l}: {e}"))
        })
        .collect();

    // session_before_compact arrived (the fixture recorded the preparation).
    let before = entries
        .iter()
        .find(|e| e["type"] == "custom" && e["customType"] == "wp704-before-compact")
        .expect("session_before_compact observable missing\n{file}");
    assert_eq!(before["data"]["reason"], "threshold");
    assert_eq!(before["data"]["willRetry"], json!(false));
    assert_eq!(
        before["data"]["branchEntries"], 2,
        "the two pre-compaction messages: {before}"
    );

    // session_compact arrived with the saved entry.
    let after = entries
        .iter()
        .find(|e| e["type"] == "custom" && e["customType"] == "wp704-compacted")
        .expect("session_compact observable missing\n{file}");
    assert_eq!(after["data"]["summary"], "WP704 SUMMARY");
    assert_eq!(after["data"]["firstKeptEntryId"], first_kept);
    assert_eq!(after["data"]["tokensBefore"], comp["tokensBefore"]);
    assert_eq!(after["data"]["fromExtension"], json!(false));
    assert_eq!(after["data"]["reason"], "threshold");
    assert_eq!(after["data"]["willRetry"], json!(false));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-704c: a session file containing a compaction entry loads with the
/// upstream buildContextEntries projection (compactionSummary + kept tail +
/// everything after; summarized entries projected out).
#[test]
fn wp704_compaction_resume_projection() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp704c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let session_file = tmp.join("resumed.jsonl");
    let content = "\
{\"type\":\"session\",\"version\":3,\"id\":\"ses-wp704\",\"timestamp\":\"2025-03-01T10:00:00.000Z\",\"cwd\":\"/work/resume\"}
{\"type\":\"message\",\"id\":\"e1\",\"parentId\":null,\"timestamp\":\"2025-03-01T10:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"old question\"}],\"timestamp\":\"2025-03-01T10:00:01.000Z\"}}
{\"type\":\"message\",\"id\":\"e2\",\"parentId\":\"e1\",\"timestamp\":\"2025-03-01T10:00:02.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"old answer\"}],\"api\":\"fake\",\"provider\":\"fake\",\"model\":\"fake\",\"usage\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"stop\",\"timestamp\":\"2025-03-01T10:00:02.000Z\"}}
{\"type\":\"message\",\"id\":\"e3\",\"parentId\":\"e2\",\"timestamp\":\"2025-03-01T10:00:03.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"kept question\"}],\"timestamp\":\"2025-03-01T10:00:03.000Z\"}}
{\"type\":\"message\",\"id\":\"e4\",\"parentId\":\"e3\",\"timestamp\":\"2025-03-01T10:00:04.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"kept answer\"}],\"api\":\"fake\",\"provider\":\"fake\",\"model\":\"fake\",\"usage\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"stop\",\"timestamp\":\"2025-03-01T10:00:04.000Z\"}}
{\"type\":\"compaction\",\"id\":\"e5\",\"parentId\":\"e4\",\"timestamp\":\"2025-03-01T11:00:00.000Z\",\"summary\":\"RESUME SUMMARY\",\"firstKeptEntryId\":\"e3\",\"tokensBefore\":1234}
{\"type\":\"message\",\"id\":\"e6\",\"parentId\":\"e5\",\"timestamp\":\"2025-03-01T11:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"after question\"}],\"timestamp\":\"2025-03-01T11:00:01.000Z\"}}
{\"type\":\"message\",\"id\":\"e7\",\"parentId\":\"e6\",\"timestamp\":\"2025-03-01T11:00:02.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"after answer\"}],\"api\":\"fake\",\"provider\":\"fake\",\"model\":\"fake\",\"usage\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"stop\",\"timestamp\":\"2025-03-01T11:00:02.000Z\"}}
";
    std::fs::write(&session_file, content).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--prompt")
        .arg("continue the session")
        .arg("--resume")
        .arg(&session_file)
        .arg("--agent-dir")
        .arg(tmp.join("agent-dir"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let last = lines.last().expect("no transcript event");
    let msgs = last["session"]["messages"]
        .as_array()
        .expect("projected messages")
        .to_vec();
    let roles: Vec<&str> = msgs
        .iter()
        .filter_map(|m| m.get("role").and_then(Value::as_str))
        .collect();
    assert_eq!(
        roles,
        vec![
            "compactionSummary",
            "user",
            "assistant",
            "user",
            "assistant",
            "user",
            "assistant"
        ],
        "upstream buildContextEntries projection: {msgs:?}"
    );
    assert_eq!(msgs[0]["content"][0]["text"], "RESUME SUMMARY");
    let all: String = msgs.iter().map(|m| m.to_string()).collect();
    assert!(
        !all.contains("old question"),
        "summarized entries projected out: {all}"
    );
    assert!(
        !all.contains("old answer"),
        "summarized entries projected out: {all}"
    );
    assert!(
        all.contains("kept question")
            && all.contains("kept answer")
            && all.contains("after question")
    );
    assert!(
        all.contains("continue the session"),
        "new prompt appended after the projection: {all}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-704a: a session_before_compact handler that returns {cancel: true}
/// aborts the compaction (no entry written) and the turn proceeds uncompacted.
#[test]
fn wp704_before_compact_cancel() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp704d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let (scenario, _filler) = wp704_compaction_scenario();
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    cmd.arg("--extension-dir")
        .arg(repo().join("tests/fixtures/compaction-events.ts"));
    cmd.arg("--agent-dir").arg(&agent_dir);
    cmd.arg("--scenario").arg(&scen);
    cmd.arg("--session-file").arg(&session_file);
    cmd.env("WP704_CANCEL", "1"); // the fixture's cancel switch (sidecar env)
    let out = cmd.output().expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "cancelled compaction must not fail the run\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let file = std::fs::read_to_string(&session_file).expect("session file missing");
    let entries: Vec<Value> = file
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("session line not JSON: {l}: {e}"))
        })
        .collect();

    // No compaction entry, no compaction event line.
    assert!(
        !entries.iter().any(|e| e["type"] == "compaction"),
        "no compaction entry when cancelled\n{file}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.get("kind").and_then(Value::as_str) == Some("compaction")),
        "no compaction NDJSON line when cancelled\n{stdout}"
    );

    // The before-compact handler ran (it recorded the event) and cancelled.
    let before = entries
        .iter()
        .find(|e| e["type"] == "custom" && e["customType"] == "wp704-before-compact")
        .expect("session_before_compact handler did not run\n{file}");
    assert_eq!(before["data"]["reason"], "threshold");
    assert_eq!(before["data"]["cancel"], json!(true));
    assert!(
        !entries
            .iter()
            .any(|e| e["type"] == "custom" && e["customType"] == "wp704-compacted"),
        "session_compact must not fire on cancel\n{file}"
    );

    // The conversation continues uncompacted: the history is still in the transcript.
    let last = lines.last().expect("no transcript event");
    let all: String = last["session"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m.to_string())
        .collect();
    assert!(
        all.contains("first turn done"),
        "uncompacted transcript keeps the history: {all}"
    );
    assert!(
        all.contains("WP704LONG"),
        "the long user message is still in context: {all}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// S7/WP-705: a fixture extension registers a clean shortcut plus one on a
/// RESERVED built-in (ctrl+c). The sidecar resolves with the upstream
/// precedence (reserved wins -> the conflicting shortcut is skipped with a
/// diagnostic, which the core surfaces on stderr); the run itself is
/// unaffected.
#[test]
fn wp705_shortcut_registration_and_conflict_diagnostic() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp705-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let scenario = json!({
        "user_messages": ["shortcut check"],
        "model_scripts": [[{ "text": "ok" }]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/shortcut-reg.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    // The reserved conflict is reported with the upstream diagnostic text
    // (the core logs every shortcut diagnostic from the ready payload).
    assert!(
        stderr.contains("conflicts with built-in shortcut. Skipping."),
        "reserved conflict diagnostic missing:\n{stderr}"
    );
    assert!(
        stderr.contains("ctrl+c"),
        "the conflicting key is named:\n{stderr}"
    );
}

// ---- WP-1003 (protocol v5): active tool set ---------------------------------

/// A deferred-exposure tool (registered inactive) becomes model-visible mid-run
/// when an active tool calls pi.setActiveTools: the first turn must NOT
/// declare it, a later turn MUST, and it then executes.
#[test]
fn wp1003_deferred_tool_activated_mid_run() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1003-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let scenario = json!({
        "user_messages": ["find it", "echo now"],
        "model_scripts": [[
            { "tool_call": { "name": "find_deferred", "input": {} } },
            { "text": "found" }
        ], [
            { "tool_call": { "name": "deferred_echo", "input": { "text": "x" } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/tool-activation.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The tools_activated wire event reached the core (corelog on stderr).
    assert!(
        stderr.contains("[core] tools_activated:"),
        "tools_activated event missing on stderr\n{stderr}"
    );

    // Every before_agent_start decision carries selectedTools.
    let bas: Vec<&Value> = lines
        .iter()
        .filter(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("before_agent_start")
        })
        .collect();
    assert!(
        bas.len() >= 2,
        "expected >=2 before_agent_start decisions\n{stdout}"
    );
    let selected_of = |l: &Value| {
        l["request"]["systemPromptOptions"]["selectedTools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };

    // Turn 1: find_deferred is declared, deferred_echo is NOT (inactive at
    // registration: exposure deferred, defaultActive false).
    let first = selected_of(bas[0]);

    assert!(
        first.iter().any(|t| t == "find_deferred"),
        "turn 1 must declare find_deferred\n{first:?}"
    );
    assert!(
        !first.iter().any(|t| t == "deferred_echo"),
        "turn 1 must NOT declare deferred_echo (inactive)\n{first:?}"
    );

    // The last turn (after find_deferred ran) declares deferred_echo.
    let last = selected_of(bas.last().unwrap());
    assert!(
        last.iter().any(|t| t == "deferred_echo"),
        "final turn must declare deferred_echo after activation\n{last:?}"
    );

    // The deferred tool executed and produced its result.
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "deferred_echo")
        .collect();
    assert_eq!(
        execs.len(),
        1,
        "deferred_echo must execute exactly once\n{stdout}"
    );
    let summary = execs[0]["result_summary"].to_string();
    assert!(
        summary.contains("deferred-echo:x"),
        "deferred_echo result missing\n{execs:?}"
    );
}

// ---- WP-1004 (codemode hosting): sandbox execution + nested tools -----------

/// The built-in codemode extension is hosted by the sidecar: a codemode tool
/// call runs the QuickJS sandbox, and a `tools.<name>(...)` call inside the
/// script round-trips through the bridge (tool_call_request → core → the
/// normal tool_call decision → back into the sandbox).
#[test]
fn wp1004_codemode_runs_script_and_nested_tool() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1004-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let scenario = json!({
        "user_messages": ["run the script"],
        "model_scripts": [[
            { "tool_call": { "name": "codemode", "input": { "code": "const r = await tools.echo_param({ text: \"x\" }); return JSON.stringify(r) + \" | 1+1=\" + (1 + 1);" } } },
            { "text": "done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(repo().join("tests/fixtures/codemode-nested.ts"))
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The nested call went through the normal tool_call decision path.
    let nested_decisions: Vec<&Value> = lines
        .iter()
        .filter(|l| {
            l.get("kind").and_then(Value::as_str) == Some("decision")
                && l.get("name").and_then(Value::as_str) == Some("tool_call")
                && l.get("request")
                    .and_then(|r| r.get("toolName"))
                    .and_then(Value::as_str)
                    == Some("echo_param")
        })
        .collect();
    assert_eq!(
        nested_decisions.len(),
        1,
        "exactly one nested tool_call decision for echo_param\n{stdout}"
    );

    // The sandbox ran: the codemode result carries the nested tool's output
    // and the pure JS arithmetic.
    let execs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "tool_execution" && l["name"] == "codemode")
        .collect();
    assert_eq!(
        execs.len(),
        1,
        "codemode must execute exactly once\n{stdout}"
    );
    let summary = execs[0]["result_summary"].to_string();
    assert!(
        summary.contains("echo-param:x"),
        "nested tool result missing from the sandbox output\n{summary}"
    );
    assert!(
        summary.contains("1+1=2"),
        "sandbox JS evaluation missing\n{summary}"
    );
}

// ---- WP-1005 (native provider object form): llama-compatible capture --------

/// A native-form provider extension (object, as the unmodified llama
/// extension registers) drives a scripted full turn through the EXISTING
/// provider_stream path; the sidecar-resolved auth (env-style) is merged
/// into the stream options.
#[test]
fn wp1005_native_provider_drives_turn() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1005a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/native-provider.ts");
    let scenario = json!({
        "user_messages": ["switch"],
        "model_scripts": [[
            { "tool_call": { "name": "switch_to_native", "input": {} } },
            { "text": "should never be used" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--scenario")
        .arg(&scen)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    let exec = lines
        .iter()
        .find(|l| l["kind"] == "tool_execution" && l["name"] == "switch_to_native")
        .expect("switch_to_native tool_execution missing\n{stdout}");
    assert_eq!(exec["result_summary"], "switched:true");

    // Turn 2 was served by the native provider with the resolved auth key
    // merged into the stream options.
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2, "expected 2 model outputs\n{stdout}");
    assert_eq!(
        model_outputs[1]["text"], "native stream, key=native-key-ok",
        "resolved auth must reach the provider stream options"
    );

    let file = std::fs::read_to_string(&session_file).unwrap();
    let changes: Vec<Value> = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("model_change"))
        .collect();
    assert_eq!(changes.len(), 1, "model_change entries: {file}");
    assert_eq!(changes[0]["provider"], "fakenative");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A native provider whose env-based auth does not resolve must produce a
/// clean typed error (exit 1, `agent run failed` with the reason), not a
/// crash — the llama "router not configured" case.
#[test]
fn wp1005_native_provider_unconfigured_is_typed_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1005b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/native-unconfigured.ts");
    let scenario = json!({
        "user_messages": ["switch"],
        "model_scripts": [[
            { "tool_call": { "name": "switch_to_unconfigured", "input": {} } },
            { "text": "should never be used" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
    cmd.arg("--extension-dir")
        .arg(&fixture)
        .arg("--scenario")
        .arg(&scen)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file);
    // Ensure the fixture's env var is unset even if the outer env has it.
    cmd.env_remove("FAKE_UNCONFIGURED_URL");
    let out = cmd.output().expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // NDJSON mode: a run failure (fail set) exits 2 (degraded would be 3).
    assert_eq!(
        out.status.code(),
        Some(2),
        "typed provider failure must exit 2 in ndjson mode\n{stderr}"
    );
    assert!(
        stderr.contains("agent run failed: provider fakeunconfigured is not configured"),
        "typed not-configured error missing\n{stderr}"
    );

    // WP-610: ndjson mode keeps stdout clean on run errors — the typed error
    // is on stderr + exit 2 (asserted above); the scripted turn must not leak.
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert!(
        !model_outputs
            .iter()
            .any(|m| m["text"].as_str() == Some("should never be used")),
        "scripted turn leaked: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The UNMODIFIED built-in llama extension: with LLAMA_BASE_URL pointing at
/// the mock router, refreshModels populates the catalog before `ready`, and
/// a turn routed to the llama model streams from the router through the
/// native provider object (env auth: key defaults to "local").
#[test]
fn wp1005_unmodified_llama_via_mock_router() {
    let mut child = Command::new("node")
        .arg(repo().join("tests/fixtures/llama-router-mock.mjs"))
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning mock llama router");
    let mut port_reader = std::io::BufReader::new(child.stdout.take().expect("mock router stdout"));
    let mut port_line = String::new();
    port_reader
        .read_line(&mut port_line)
        .expect("mock router port line");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("mock router port");

    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1005c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = repo().join("tests/fixtures/switch-to-llama.ts");
    let scenario = json!({
        "user_messages": ["switch"],
        "model_scripts": [[
            { "tool_call": { "name": "switch_to_llama", "input": {} } },
            { "text": "should never be used" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--extension-dir")
        .arg(&fixture)
        .arg("--scenario")
        .arg(&scen)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .env("LLAMA_BASE_URL", format!("http://127.0.0.1:{port}"))
        .output()
        .expect("running pi-rust");
    let _ = child.kill();
    let _ = child.wait();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);

    // The model switch to the router-discovered llama model was accepted.
    let exec = lines
        .iter()
        .find(|l| l["kind"] == "tool_execution" && l["name"] == "switch_to_llama")
        .expect("switch_to_llama tool_execution missing\n{stdout}");
    assert_eq!(
        exec["result_summary"], "switched:true",
        "catalog from the mock router must be registered before the switch"
    );

    // Turn 2 streamed from the mock router (the echo proves the transcript
    // reached /v1/chat/completions).
    let model_outputs: Vec<&Value> = lines
        .iter()
        .filter(|l| l["kind"] == "model_output")
        .collect();
    assert_eq!(model_outputs.len(), 2, "expected 2 model outputs\n{stdout}");
    assert_eq!(model_outputs[1]["text"], "llama-mock: switch");

    let file = std::fs::read_to_string(&session_file).unwrap();
    let changes: Vec<Value> = file
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("model_change"))
        .collect();
    assert_eq!(changes.len(), 1, "model_change entries: {file}");
    assert_eq!(changes[0]["provider"], "llama.cpp");
    assert_eq!(changes[0]["modelId"], "gemma-mock");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1006 headless contract: without the `custom` capability (headless
/// init advertises ui: []), ctx.ui.custom resolves with the upstream noOp
/// result (undefined) — the factory never runs, nothing hangs, no
/// pane_* events cross the wire. The fixture command throws on any
/// deviation (ok:false on the command event).
#[test]
fn wp1006_headless_custom_is_noop() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1006-headless-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let ext = repo().join("tests/fixtures");
    let agent_dir = tmp.join("agent-dir");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("fake")
        .arg("--extension-dir")
        .arg(&ext)
        .arg("--command")
        .arg("custom-probe")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--prompt")
        .arg("probe")
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let cmd = lines
        .iter()
        .find(|l| l.get("kind").and_then(Value::as_str) == Some("command"))
        .expect("command event missing\n{stdout}");
    assert_eq!(cmd["name"], "custom-probe");
    assert_eq!(
        cmd["ok"],
        json!(true),
        "custom-probe must pass (noOp undefined)\n{cmd}"
    );

    // No pane lifecycle events may appear in the core log.
    let corelog = std::fs::read_to_string(agent_dir.join("core.log")).unwrap_or_default();
    assert!(
        !corelog.contains("pane_open")
            && !corelog.contains("pane_update")
            && !corelog.contains("pane_key"),
        "headless runs must not see pane_* events\n{corelog}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1007 (re-pointed by E3S4/WP-1404): an expired stored oauth credential
/// (kimi-coding) is refreshed per upstream semantics before the provider
/// call — now CORE-side (the core owns kimi-coding end-to-end, D15): the
/// refresh + rotated write-back run in the Rust core, and the stream goes
/// through the core's built-in OpenAI driver. The token endpoint is mocked
/// via KIMI_CODE_OAUTH_HOST and the chat call via KIMI_BASE_URL (all local
/// mocks — no real provider endpoint, the WP-1007 original hit
/// api.kimi.com); the sidecar is ALIVE but no longer routes the provider,
/// so a successful run is also the coexistence proof. Asserted: the refresh
/// request used the stored refresh token, auth.json holds the rotated
/// credential, the transcript carries the mock's text (typed run, exit 0),
/// and no token material lands in any output or log.
#[test]
fn wp1007_oauth_credential_refreshes_before_provider_call() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1007-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (oauth_host, refresh_log, mut child) = start_kimi_oauth_mock(&tmp);
    let (chat_base, _chat) = sse_no_finish_mock("Kimi refresh run ok");

    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Pre-seed an EXPIRED oauth credential (expires in the past; the core
    // treats <5min remaining as expired, upstream margin).
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"kimi-coding":{"type":"oauth","access":"old-access","refresh":"old-refresh","expires":1}}"#,
    )
    .unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .env("KIMI_CODE_OAUTH_HOST", &oauth_host)
        .env("KIMI_BASE_URL", &chat_base)
        .output()
        .expect("running pi-rust");
    let _ = child.kill();
    let _ = child.wait();

    // 1) The refresh went to the mock with the stored refresh token.
    let log = std::fs::read_to_string(&refresh_log).unwrap_or_default();
    assert!(
        log.contains("old-refresh"),
        "the mock token endpoint saw no refresh with the stored token\n{log}"
    );
    assert!(
        log.contains("grant_type=refresh_token"),
        "upstream grant type\n{log}"
    );

    // 2) The rotated credential is written back to auth.json (upstream shape).
    let auth: Value =
        serde_json::from_str(&std::fs::read_to_string(agent_dir.join("auth.json")).unwrap())
            .unwrap();
    let cred = &auth["kimi-coding"];
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("new-access"), "rotated access token");
    assert_eq!(
        cred["refresh"],
        json!("new-refresh"),
        "rotated refresh token"
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        cred["expires"].as_i64().unwrap_or(0) > now_ms,
        "expires must be in the future: {cred}"
    );

    // 3) The run succeeds against the mock chat endpoint (core driver; the
    //    sidecar is alive but no longer routes the provider).
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "core-native run against the mock chat\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!stderr.contains("panic"), "no crash\n{stderr}");
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Kimi refresh run ok"),
        "the mock's text in the transcript\n{dump}"
    );

    // 4) Redaction: no token material in outputs or logs (WP-904 rule).
    let combined = format!("{stdout}\n{stderr}");
    let corelog = std::fs::read_to_string(agent_dir.join("core.log")).unwrap_or_default();
    for secret in ["old-access", "old-refresh", "new-access", "new-refresh"] {
        assert!(
            !combined.contains(secret),
            "token material in output: {secret}"
        );
        assert!(
            !corelog.contains(secret),
            "token material in core.log: {secret}"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1102 (D2): a project extension under <cwd>/.pi/extensions/ loads only
/// when the project is trusted. Default resolution for a trust-requiring
/// project in headless mode ("ask" + no UI) = not trusted.
#[test]
fn project_extensions_gated_on_trust() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-trust-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    std::fs::create_dir_all(proj.join(".pi/extensions")).unwrap();
    std::fs::copy(
        repo().join("tests/fixtures/trust-marker.ts"),
        proj.join(".pi/extensions/trust-marker.ts"),
    )
    .unwrap();
    let scenario = json!({ "user_messages": ["hi"], "model_scripts": [[{ "text": "done" }]] });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent-dir");
    let session_file = tmp.join("session.jsonl");

    let run = |extra: &[&std::ffi::OsStr]| -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
        cmd.current_dir(&proj)
            .arg("--agent-dir")
            .arg(&agent_dir)
            .arg("--scenario")
            .arg(&scen)
            .arg("--session-file")
            .arg(&session_file);
        for a in extra {
            cmd.arg(a);
        }
        let out = cmd.output().expect("running pi-rust");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(
            out.status.code(),
            Some(0),
            "pi-rust exited non-zero\n{stderr}"
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    // Untrusted (headless "ask" = deny): the project extension never ran.
    let untrusted = run(&[]);
    assert!(
        !untrusted.contains("TRUST-MARKER-SENTINEL"),
        "project extension took effect in an untrusted project"
    );

    // --approve: the project extension is loaded and its system-prompt
    // injection lands in the before_agent_start decision.
    let trusted = run(&[std::ffi::OsStr::new("--approve")]);
    assert!(
        trusted.contains("TRUST-MARKER-SENTINEL"),
        "project extension missing with --approve"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1301/E2S1, re-pointed by WP-1402/E3S2: a custom provider defined in
/// the agent dir's `models.json` (upstream format — the solheim field case:
/// JSONC comment, `$ENV` apiKey) works end-to-end with NO extension shim —
/// and the model is owned by the CORE (E3S1, D11): the core parses
/// models.json natively and streams it through the built-in OpenAI driver
/// even with the sidecar alive; the sidecar no longer advertises or routes
/// models.json providers (E3S2). The mock's final chunk carries NO
/// `finish_reason` (pi-ai's openai-completions streamer — the old sidecar
/// path — requires it, the core's lenient driver does not), so a transcript
/// carrying the mock's text proves the core's driver served the call.
#[test]
fn models_json_custom_provider_end_to_end() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1301-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Mock OpenAI SSE endpoint (no finish_reason — see the doc comment).
    let (base, _server) = sse_no_finish_mock("Hello from models.json");

    // The agent dir with a models.json custom provider (JSONC, $ENV apiKey
    // — the solheim field case).
    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{
  // e2e custom provider (WP-1301)
  "providers": {
    "mockcustom": {
      "name": "Mock Custom",
      "baseUrl": "__BASE__",
      "api": "openai-completions",
      "apiKey": "$E2E_MODELS_JSON_KEY",
      "models": [{ "id": "mock-model", "name": "Mock Model", "input": ["text"], "contextWindow": 4096, "maxTokens": 512 }]
    }
  }
}
"#
        .replace("__BASE__", &base),
    )
    .unwrap();

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("E2E_MODELS_JSON_KEY", "e2e-key")
        .arg("--provider")
        .arg("mockcustom")
        .arg("--model")
        .arg("mock-model")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("stdout line not JSON: {l}: {e}"))
        })
        .collect();
    let final_line = lines.last().expect("no events");
    assert_eq!(final_line["kind"], "transcript");
    let dump = final_line["session"].to_string();
    assert!(
        dump.contains("Hello from models.json"),
        "mockcustom/models.json response missing from transcript (no-finish mock: only the core's native driver can serve it)\n{dump}"
    );
    assert!(
        !dump.contains("not a built-in provider"),
        "provider resolution must not fall back to the typed fatal\n{dump}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// --- WP-1401 / E3S1: core-native models.json + OpenAI-compatible streaming ---

/// A mock OpenAI SSE endpoint (same response per connection) whose final
/// chunk carries NO `finish_reason` — the core's OpenAI driver is lenient
/// about it, but pi-ai's openai-completions streamer (the old sidecar path,
/// removed in E3S2) requires it and fails the stream. So a transcript
/// carrying the mock's text PROVES the core's native driver served the call
/// — even when the sidecar is alive (E3S2: it no longer advertises the
/// provider at all, so no bridge route exists).
fn sse_no_finish_mock(text: &str) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let text = text.to_string();
    // Detached: the thread parks on accept() and dies with the test process;
    // every connection gets the same response (a run may call the provider
    // once per selection variant).
    let server = std::thread::spawn(move || {
        use std::io::Read;
        loop {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            // Read until the header terminator appears anywhere in what we
            // have (reqwest delivers headers+body in one segment).
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
            let c1 = json!({"choices":[{"delta":{"role":"assistant","content":text}}]}).to_string();
            let sse = format!("data: {c1}\ndata: [DONE]\n");
            let mut body =
                String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
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
            use std::io::Write;
            let _ = s.write_all(body.as_bytes());
        }
    });
    (base, server)
}

/// WP-1401 / E3S1 (HARD acceptance): the solheim case — a models.json
/// provider against a mock OpenAI SSE endpoint — works with the sidecar
/// DEGRADED (spawn failure): the provider call is served by the core's
/// built-in OpenAI driver and never crosses the bridge (there is no
/// bridge — the sidecar never spawned).
#[test]
fn models_json_core_native_sidecar_degraded() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1401-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (base, _server) = sse_no_finish_mock("Hello from models.json");

    // The agent dir with a models.json custom provider (JSONC, $ENV apiKey —
    // the solheim field case).
    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{
  // e2e core-native provider (WP-1401)
  "providers": {
    "mockcustom": {
      "name": "Mock Custom",
      "baseUrl": "__BASE__",
      "api": "openai-completions",
      "apiKey": "$E3S1_MODELS_JSON_KEY",
      "models": [{ "id": "mock-model", "name": "Mock Model", "input": ["text"], "contextWindow": 4096, "maxTokens": 512 }]
    }
  }
}
"#
        .replace("__BASE__", &base),
    )
    .unwrap();

    let run = |extra: &[&std::ffi::OsStr]| -> (String, String) {
        let session_file = tmp.join(format!("session-{}.jsonl", uuid::Uuid::new_v4().simple()));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pi-rust"));
        // Sidecar degraded: a spawn failure (the node binary does not
        // exist). The run must continue with the core-native driver.
        cmd.env("PI_RUST_NODE", "/nonexistent/pi-rust-e3s1-node")
            .env("E3S1_MODELS_JSON_KEY", "e3s1-key")
            .arg("--provider")
            .arg("mockcustom")
            .arg("--model")
            .arg("mock-model")
            .arg("--prompt")
            .arg("hi")
            .arg("--agent-dir")
            .arg(&agent_dir)
            .arg("--session-file")
            .arg(&session_file);
        for a in extra {
            cmd.arg(a);
        }
        let out = cmd.output().expect("running pi-rust");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(
            out.status.code(),
            Some(0),
            "pi-rust exited non-zero\n{stderr}"
        );
        (String::from_utf8_lossy(&out.stdout).to_string(), stderr)
    };

    // (1) Flag selection — the E2S1 e2e's same selection, sidecar degraded.
    let (stdout, stderr) = run(&[]);
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path\n{stderr}"
    );
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert_eq!(lines.last().unwrap()["kind"], "transcript");
    assert!(
        dump.contains("Hello from models.json"),
        "the core's native driver must have served the call (no bridge — no sidecar)\n{dump}"
    );
    assert!(
        !dump.contains("not a built-in provider"),
        "provider resolution must not fall back to the typed fatal\n{dump}"
    );

    // (2) settings.json selection — same behavior via
    // defaultProvider/defaultModel (the E2S1 parity entry point).
    std::fs::write(
        agent_dir.join("settings.json"),
        r#"{"defaultProvider":"mockcustom","defaultModel":"mock-model"}"#,
    )
    .unwrap();
    let session_file = tmp.join(format!(
        "session-set-{}.jsonl",
        uuid::Uuid::new_v4().simple()
    ));
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e3s1-node")
        .env("E3S1_MODELS_JSON_KEY", "e3s1-key")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust (settings selection)");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "\n{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Hello from models.json"),
        "settings defaultProvider/defaultModel selection must work degraded\n{dump}"
    );

    // The mock thread is detached (it parks on accept() until the test
    // process exits); the child served everything before the assertions.
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1401/E3S1: with the sidecar ALIVE (and, since WP-1402/E3S2, no
/// longer advertising the models.json provider in `ready.providers`), the
/// core routes its own models.json models to the NATIVE driver: the mock's
/// final chunk has no `finish_reason`, which the sidecar's upstream
/// streamer would reject — so the transcript carrying it proves the call
/// never crossed the bridge (E3S2: the sidecar now has no models.json
/// route at all — an off-route `provider_stream` is rejected).
#[test]
fn models_json_core_native_sidecar_healthy_still_native() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1401h-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (base, _server) = sse_no_finish_mock("Hello from models.json");

    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{"providers":{"mockcustom":{"name":"Mock Custom","baseUrl":"__BASE__","api":"openai-completions","apiKey":"$E3S1_MODELS_JSON_KEY","models":[{"id":"mock-model","name":"Mock Model","contextWindow":4096,"maxTokens":512}]}}}
"#
        .replace("__BASE__", &base),
    )
    .unwrap();

    let session_file = tmp.join("session.jsonl");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("E3S1_MODELS_JSON_KEY", "e3s1-key")
        .arg("--provider")
        .arg("mockcustom")
        .arg("--model")
        .arg("mock-model")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "pi-rust exited non-zero\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Hello from models.json"),
        "a live sidecar must not pull the core's own models.json models over the bridge\n\n{dump}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1401 / E3S1: other api types on models.json providers are a typed
/// "not core-native yet" error — no crash, no provider call.
#[test]
fn models_json_non_native_api_is_typed_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1401a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("models.json"),
        r#"{"providers":{"anthro":{"name":"Anthro","baseUrl":"http://127.0.0.1:1","api":"anthropic-messages","apiKey":"k","models":[{"id":"m1"}]}}}
"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        // The error fires at driver build, before the sidecar matters;
        // keep the run fast and hermetic with a dead node binary.
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-e3s1-node")
        .arg("--provider")
        .arg("anthro")
        .arg("--model")
        .arg("m1")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(1),
        "a typed startup error is exit 1\n{stderr}"
    );
    assert!(
        stderr.contains("not core-native yet"),
        "typed api rejection missing\n{stderr}"
    );
    assert!(
        !stderr.to_lowercase().contains("panic"),
        "no crash: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1401 / E3S1: a malformed models.json is a typed error and the run
/// continues with the bundled catalog (here: the scripted fake driver).
#[test]
fn models_json_malformed_run_continues_with_bundled() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1401m-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let scenario = json!({ "user_messages": ["hi"], "model_scripts": [[{ "text": "ok" }]] });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let agent_dir = tmp.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("models.json"), "{not json").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--scenario")
        .arg(&scen)
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(tmp.join("session.jsonl"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "malformed models.json must not kill the run\n{stderr}"
    );
    assert!(
        stderr.contains("models.json") && stderr.contains("invalid JSON"),
        "the typed error must be logged\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(dump.contains("ok"), "transcript: {dump}");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Like `sse_no_finish_mock`, but records the `authorization` header of
/// every request into `log_file` (one line per request) — E3S4: proves the
/// core's driver sent the stored/rotated credential (or the env key).
fn sse_no_finish_mock_recording(
    text: &str,
    log_file: &std::path::Path,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let text = text.to_string();
    let log_file = log_file.to_path_buf();
    let server = std::thread::spawn(move || {
        use std::io::Read;
        loop {
            let Ok((mut s, _)) = listener.accept() else {
                return;
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
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let auth = head
                .lines()
                .map(str::to_lowercase)
                .find_map(|l| l.strip_prefix("authorization:").map(str::to_string))
                .map(|a| a.trim().to_string());
            if let Some(auth) = auth {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_file)
                {
                    let _ = std::io::Write::write_all(&mut f, format!("{auth}\n").as_bytes());
                }
            }
            let c1 = json!({"choices":[{"delta":{"role":"assistant","content":text}}]}).to_string();
            let sse = format!("data: {c1}\ndata: [DONE]\n");
            let mut body =
                String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
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
            use std::io::Write;
            let _ = s.write_all(body.as_bytes());
        }
    });
    (base, server)
}

/// Spawns the kimi oauth token-endpoint mock (tests/fixtures/kimi-oauth-mock.mjs)
/// and returns (base URL, log file path, child). The token endpoint answers
/// rotated tokens and logs every request body (WP-1007 fixture; the E3S4
/// device_authorization extension serves the login flow as well).
fn start_kimi_oauth_mock(
    tmp: &std::path::Path,
) -> (String, std::path::PathBuf, std::process::Child) {
    let refresh_log = tmp.join("refresh-log.jsonl");
    let mut child = Command::new("node")
        .arg(repo().join("tests/fixtures/kimi-oauth-mock.mjs"))
        .arg(&refresh_log)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning mock kimi oauth token endpoint");
    let mut port_reader = std::io::BufReader::new(child.stdout.take().expect("mock stdout"));
    let mut port_line = String::new();
    port_reader
        .read_line(&mut port_line)
        .expect("mock port line");
    let port: u16 = port_line
        .trim()
        .rsplit(' ')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("mock port");
    (format!("http://127.0.0.1:{port}"), refresh_log, child)
}

/// WP-1404 / E3S4 (HARD acceptance): the kimi-coding oauth builtin runs
/// through the CORE with the sidecar DEGRADED (spawn failure) — the core
/// refreshes the expired stored credential (mock token endpoint) and streams
/// the call through its built-in OpenAI driver (mock chat endpoint, NO
/// `finish_reason` — the sidecar's upstream streamer would reject it, so the
/// transcript carrying the text proves the call never crossed the bridge).
/// All traffic local: token endpoint + chat endpoint are mocks.
#[test]
fn wp1404_kimi_oauth_native_sidecar_degraded() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1404-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (oauth_host, refresh_log, mut child) = start_kimi_oauth_mock(&tmp);
    let (chat_base, _chat) =
        sse_no_finish_mock_recording("Kimi hello from the core", &tmp.join("auth-headers.txt"));

    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    // Pre-seed an EXPIRED oauth credential (expires in the past; the core
    // treats <5min remaining as expired, upstream margin) + an unrelated
    // provider entry that the rotated write-back must preserve.
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"kimi-coding":{"type":"oauth","access":"old-access","refresh":"old-refresh","expires":1},"groq":{"type":"api_key","key":"groq-key"}}"#,
    )
    .unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        // Sidecar degraded: the node binary does not exist — the run must be
        // served by the core alone (login/refresh/stream, no bridge).
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1404-node")
        .env("KIMI_CODE_OAUTH_HOST", &oauth_host)
        .env("KIMI_BASE_URL", &chat_base)
        .output()
        .expect("running pi-rust");
    let _ = child.kill();
    let _ = child.wait();

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    // 1) The run went through the degraded-sidecar path and still succeeded:
    // the core's native driver served the provider call.
    assert_eq!(out.status.code(), Some(0), "exit\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "the run must go through the degraded-sidecar path\n{stderr}"
    );
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Kimi hello from the core"),
        "the core's native driver must have served the call (no sidecar)\n{dump}"
    );

    // 2) The refresh went to the mock with the stored refresh token.
    let log = std::fs::read_to_string(&refresh_log).unwrap_or_default();
    assert!(
        log.contains("old-refresh"),
        "no refresh with the stored token\n{log}"
    );
    assert!(
        log.contains("grant_type=refresh_token"),
        "upstream grant type\n{log}"
    );

    // 3) The rotated credential is written back to auth.json (upstream shape;
    // other providers preserved).
    let auth: Value =
        serde_json::from_str(&std::fs::read_to_string(agent_dir.join("auth.json")).unwrap())
            .unwrap();
    let cred = &auth["kimi-coding"];
    assert_eq!(cred["type"], json!("oauth"));
    assert_eq!(cred["access"], json!("new-access"), "rotated access token");
    assert_eq!(
        cred["refresh"],
        json!("new-refresh"),
        "rotated refresh token"
    );
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        cred["expires"].as_i64().unwrap_or(0) > now_ms,
        "expires must be in the future: {cred}"
    );
    assert_eq!(
        auth["groq"]["key"],
        json!("groq-key"),
        "write-back must preserve other providers"
    );

    // 4) The call carried the ROTATED token as bearer.
    let headers = std::fs::read_to_string(tmp.join("auth-headers.txt")).unwrap_or_default();
    assert!(
        headers.contains("bearer new-access"),
        "rotated token used for the call\n{headers}"
    );

    // 5) Redaction: no token material in outputs or logs (WP-904 rule).
    let combined = format!("{stdout}\n{stderr}");
    let corelog = std::fs::read_to_string(agent_dir.join("core.log")).unwrap_or_default();
    for secret in ["old-access", "old-refresh", "new-access", "new-refresh"] {
        assert!(
            !combined.contains(secret),
            "token material in output: {secret}"
        );
        assert!(
            !corelog.contains(secret),
            "token material in core.log: {secret}"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1404 / E3S4 (coexistence, E3S2 pattern): with the sidecar ALIVE the
/// core still owns kimi-coding — the sidecar no longer advertises it in
/// `ready.providers`, so the core routes the model to the native driver. The
/// mock's final chunk has no `finish_reason` (the sidecar's upstream streamer
/// requires it), so a transcript carrying the text proves the call never
/// crossed the bridge even though one exists.
#[test]
fn wp1404_kimi_oauth_native_sidecar_healthy_still_native() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1404h-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (oauth_host, refresh_log, mut child) = start_kimi_oauth_mock(&tmp);
    let (chat_base, _chat) = sse_no_finish_mock("Kimi hello with sidecar alive");

    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"kimi-coding":{"type":"oauth","access":"old-access","refresh":"old-refresh","expires":1}}"#,
    )
    .unwrap();
    let session_file = tmp.join("session.jsonl");

    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(&session_file)
        .env("KIMI_CODE_OAUTH_HOST", &oauth_host)
        .env("KIMI_BASE_URL", &chat_base)
        .output()
        .expect("running pi-rust");
    let _ = child.kill();
    let _ = child.wait();

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "\n{stdout}\n{stderr}");
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Kimi hello with sidecar alive"),
        "the core's native driver must serve kimi-coding even with the sidecar alive\n{dump}"
    );
    // The core refreshed (not the sidecar — it no longer routes the provider).
    let log = std::fs::read_to_string(&refresh_log).unwrap_or_default();
    assert!(
        log.contains("old-refresh"),
        "core-owned refresh with the stored token\n{log}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// WP-1404 / E3S4: no credential + no env key is a TYPED error pointing at
/// the core-native login (before the run starts, degraded or not); the
/// `KIMI_API_KEY` env fallback (upstream `envApiKeyAuth`) streams through the
/// core driver with the env key as bearer.
#[test]
fn wp1404_kimi_oauth_no_credential_typed_error() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1404nc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let (chat_base, _chat) =
        sse_no_finish_mock_recording("Kimi hello with env key", &tmp.join("auth-headers.txt"));
    let agent_dir = tmp.join("agent-dir");
    std::fs::create_dir_all(&agent_dir).unwrap();

    // (a) No auth.json entry, no env key: typed fatal at startup.
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(tmp.join("session-a.jsonl"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1404-node")
        .env("KIMI_BASE_URL", &chat_base)
        .env_remove("KIMI_API_KEY")
        .output()
        .expect("running pi-rust (no credential)");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(1), "typed fatal exit\n{stderr}");
    assert!(
        stderr.contains("pi-rust auth login kimi-coding"),
        "the error must point at the core-native login\n{stderr}"
    );

    // (b) The env key fallback (upstream envApiKeyAuth order: nothing stored ->
    // ambient env) streams through the core driver.
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent_dir)
        .arg("--session-file")
        .arg(tmp.join("session-b.jsonl"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1404-node")
        .env("KIMI_BASE_URL", &chat_base)
        .env("KIMI_API_KEY", "env-kimi-key")
        .output()
        .expect("running pi-rust (env key)");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "\n{stderr}");
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("Kimi hello with env key"),
        "the env-key fallback must stream through the core driver\n{dump}"
    );
    let headers = std::fs::read_to_string(tmp.join("auth-headers.txt")).unwrap_or_default();
    assert!(
        headers.contains("bearer env-kimi-key"),
        "the env key used as bearer\n{headers}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// ============================================================================
// E3S5/WP-1405: the E3 end-state regression — a Node-free run (sidecar
// spawn failing) carries all three core-native capabilities:
// (1) a models.json model streams through the core's built-in OpenAI driver,
// (2) an MCP tool is in the core's tool set (listed), callable, and its
//     result lands in the transcript,
// (3) an oauth builtin (kimi-coding, mocked stored token, refreshed by the
//     core) streams through the built-in driver.
// ============================================================================

/// HARD: one test, three sub-runs — each with `PI_RUST_NODE` pointing at a
/// nonexistent binary (spawn failure, the run goes through the degraded-
/// sidecar path) — proving the Node-free run carries providers + MCP + oauth.
#[test]
fn wp1405_sidecar_degraded_full_regression() {
    let tmp = std::env::temp_dir().join(format!("pi-rust-wp1405-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // (1) providers: a models.json model streams. The mock's final chunk has
    // no `finish_reason` (the sidecar's upstream streamer would require it —
    // proof the call never crosses a bridge).
    let (base, _chat) = sse_no_finish_mock("E3S5 providers ok");
    let agent1 = tmp.join("agent-1");
    std::fs::create_dir_all(&agent1).unwrap();
    std::fs::write(
        agent1.join("models.json"),
        r#"{"providers":{"mockcustom":{"name":"Mock Custom","baseUrl":"__BASE__","api":"openai-completions","apiKey":"$E3S5_MODELS_JSON_KEY","models":[{"id":"mock-model","name":"Mock Model","contextWindow":4096,"maxTokens":512}]}}}"#
            .replace("__BASE__", &base),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1405-node")
        .env("E3S5_MODELS_JSON_KEY", "e3s5-key")
        .arg("--provider")
        .arg("mockcustom")
        .arg("--model")
        .arg("mock-model")
        .arg("--prompt")
        .arg("hi")
        .arg("--agent-dir")
        .arg(&agent1)
        .arg("--session-file")
        .arg(tmp.join("session-1.jsonl"))
        .output()
        .expect("running pi-rust (providers)");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "(1) exit\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "(1) the run must go through the degraded-sidecar path\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("E3S5 providers ok"),
        "(1) the models.json model must stream through the core driver\n{dump}"
    );

    // (2) MCP: a stdio server from the agent dir's mcp.json — the
    // direct-exposure tool is in the core's tool set (listed), callable,
    // and its result lands in the transcript (executed natively, no bridge
    // hop). In a degraded run the model declaration is not observable on
    // the wire (no sidecar decision); a call for a tool outside the core's
    // set would fail the run as a model-script error, so the native
    // execution below is the listing proof. The declared set itself is
    // pinned with the sidecar alive (wp1403_mcp_exposure_rules_and_change
    // _event).
    let agent2 = tmp.join("agent-2");
    std::fs::create_dir_all(&agent2).unwrap();
    let fixture = repo().join("tests/fixtures/mcp-server-fixture.mjs");
    std::fs::write(
        agent2.join("mcp.json"),
        json!({
            "mcpServers": {
                "fixture": { "command": "node", "args": [fixture.to_string_lossy()], "exposure": "direct" }
            }
        })
        .to_string(),
    )
    .unwrap();
    let scenario = json!({
        "user_messages": ["use the mcp tools"],
        "model_scripts": [[
            { "tool_call": { "name": "mcp__fixture__echo", "input": { "text": "e3s5" } } },
            { "text": "mcp done" }
        ]]
    });
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, scenario.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1405-node")
        .arg("--agent-dir")
        .arg(&agent2)
        .arg("--scenario")
        .arg(&scen)
        .arg("--session-file")
        .arg(tmp.join("session-2.jsonl"))
        .output()
        .expect("running pi-rust (mcp)");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "(2) exit\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "(2) the run must go through the degraded-sidecar path\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let lines = event_log(&stdout);
    // listed + callable: native execution with the fixture's result (an
    // unknown tool would have failed the run before this point).
    let exec = lines
        .iter()
        .find(|l| l["kind"] == "tool_execution" && l["name"] == "mcp__fixture__echo")
        .expect("mcp__fixture__echo execution missing\n{stdout}");
    assert_eq!(exec["blocked"], json!(false));
    assert_eq!(
        exec["result_summary"], "echo:e3s5",
        "mcp round-trip: {exec}"
    );
    // result lands in the transcript.
    let last = lines.last().expect("no stdout lines");
    assert_eq!(last["kind"], "transcript");
    let entries: Vec<&Value> = last["session"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("message"))
        .collect();
    let roles: Vec<&str> = entries
        .iter()
        .map(|e| e["message"]["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "toolResult", "assistant"]);
    for r in entries
        .iter()
        .filter(|e| e["message"]["role"] == "toolResult")
    {
        assert_eq!(r["message"]["isError"], json!(false));
    }

    // (3) oauth: kimi-coding with a mocked stored token (expired → the core
    // refreshes it against the mock, upstream margin) streams through the
    // built-in driver.
    let (oauth_host, refresh_log, mut child) = start_kimi_oauth_mock(&tmp);
    let (chat_base, _chat2) = sse_no_finish_mock("E3S5 oauth ok");
    let agent3 = tmp.join("agent-3");
    std::fs::create_dir_all(&agent3).unwrap();
    std::fs::write(
        agent3.join("auth.json"),
        r#"{"kimi-coding":{"type":"oauth","access":"old-access","refresh":"old-refresh","expires":1}}"#,
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .arg("--provider")
        .arg("kimi-coding")
        .arg("--model")
        .arg("k3")
        .arg("--prompt")
        .arg("hello")
        .arg("--agent-dir")
        .arg(&agent3)
        .arg("--session-file")
        .arg(tmp.join("session-3.jsonl"))
        .env("PI_RUST_NODE", "/nonexistent/pi-rust-wp1405-node")
        .env("KIMI_CODE_OAUTH_HOST", &oauth_host)
        .env("KIMI_BASE_URL", &chat_base)
        .output()
        .expect("running pi-rust (oauth)");
    let _ = child.kill();
    let _ = child.wait();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "(3) exit\n{stdout}\n{stderr}");
    assert!(
        stderr.contains("sidecar spawn failed"),
        "(3) the run must go through the degraded-sidecar path\n{stderr}"
    );
    let lines = event_log(&stdout);
    let dump = lines.last().expect("no events")["session"].to_string();
    assert!(
        dump.contains("E3S5 oauth ok"),
        "(3) the oauth builtin must stream through the core driver\n{dump}"
    );
    // the core refreshed the stored credential (no sidecar involved).
    let log = std::fs::read_to_string(&refresh_log).unwrap_or_default();
    assert!(
        log.contains("old-refresh"),
        "(3) core-side refresh with the stored token\n{log}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
