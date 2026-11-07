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
