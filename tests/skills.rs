//! WP-801 end-to-end: skills discovered from `<agentDir>/skills` +
//! `<cwd>/.pi/skills`, invalid skills skipped with a warning, the golden
//! `<skills>` section in the model's system prompt (captured by a
//! before_agent_start extension), and headless `/skill:` expansion.

use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pi-rust-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_skill(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
}

/// The fixture extension dumps the systemPrompt it receives in
/// before_agent_start (upstream: the event carries the current prompt).
const CAPTURE_EXT: &str = r#"
import { writeFileSync } from "node:fs";

export default function (pi: any) {
  pi.on("before_agent_start", (event: any) => {
    const out = process.env.SKILL_PROMPT_DUMP;
    if (out) writeFileSync(out, String(event.systemPrompt ?? ""));
  });
}
"#;

/// Golden: the system prompt handed to the model carries the upstream
/// `<skills>` section for the discovered skills (agent + project dirs).
#[test]
fn wp801_skill_block_in_system_prompt() {
    let tmp = tmp("wp801p");
    write_skill(&tmp.join("agent"), "skills/greet/SKILL.md", "---\nname: greet\ndescription: say hi back\n---\nGreet the user.");
    write_skill(&tmp, ".pi/skills/proj/SKILL.md", "---\nname: proj\ndescription: project skill\n---\nBody.");
    std::fs::create_dir_all(tmp.join("ext")).unwrap();
    std::fs::write(tmp.join("ext/capture.ts"), CAPTURE_EXT).unwrap();
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, json!({"user_messages": ["hello"], "model_scripts": [[{ "text": "hi back" }]]}).to_string()).unwrap();
    let dump = tmp.join("dump.txt");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&tmp)
        .env("SKILL_PROMPT_DUMP", &dump)
        .arg("--scenario").arg(&scen)
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(tmp.join("session.jsonl"))
        .arg("--extension-dir").arg(tmp.join("ext/capture.ts"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let prompt = std::fs::read_to_string(&dump).unwrap_or_else(|e| panic!("no dump: {e} {stderr}"));
    // Base prompt preserved, then the upstream section wrapper.
    assert!(prompt.starts_with("You are pi-rust"), "{prompt}");
    assert!(prompt.contains("\n\n<skills>\nThe following skills provide specialized instructions for specific tasks."), "{prompt}");
    assert!(prompt.contains("Use the read tool to load a skill's file when the task matches its description."), "{prompt}");
    assert!(prompt.contains(&format!(
        "<skill>\n    <name>greet</name>\n    <description>say hi back</description>\n    <location>{}</location>\n  </skill>",
        tmp.join("agent/skills/greet/SKILL.md").display()
    )), "{prompt}");
    assert!(prompt.contains("<name>proj</name>\n    <description>project skill</description>"), "{prompt}");
    assert!(prompt.trim_end().ends_with("</skills>"), "{prompt}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A declared skill without a description is skipped with the upstream
/// warning; valid skills are unaffected and the run completes.
#[test]
fn wp801_invalid_skill_skipped_with_warning() {
    let tmp = tmp("wp801i");
    write_skill(&tmp.join("agent"), "skills/greet/SKILL.md", "---\nname: greet\ndescription: say hi back\n---\nGreet the user.");
    write_skill(&tmp.join("agent"), "skills/nodesc/SKILL.md", "---\nname: nodesc\n---\nno description");
    let scen = tmp.join("scenario.json");
    std::fs::write(&scen, json!({"user_messages": ["hello"], "model_scripts": [[{ "text": "hi back" }]]}).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&tmp)
        .arg("--scenario").arg(&scen)
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(tmp.join("session.jsonl"))
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("description is required") && stderr.contains("nodesc/SKILL.md"),
        "invalid skill must warn with its path: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Headless `/skill:<name> [args]` in a positional prompt expands core-side
/// (echo driver: the expanded text is echoed back in --print mode).
#[test]
fn wp801_headless_skill_expansion() {
    let tmp = tmp("wp801h");
    write_skill(&tmp.join("agent"), "skills/greet/SKILL.md", "---\nname: greet\ndescription: say hi back\n---\nGreet the user.");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&tmp)
        .arg("--print")
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(tmp.join("session.jsonl"))
        .arg("--prompt").arg("/skill:greet extra-arg")
        .output()
        .expect("running pi-rust");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let expected = format!(
        "echo: <skill name=\"greet\" location=\"{}\">\nReferences are relative to {}.\n\nGreet the user.\n</skill>\n\nextra-arg\n",
        tmp.join("agent/skills/greet/SKILL.md").display(),
        tmp.join("agent/skills/greet").display()
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(stdout, expected, "exact expanded prompt echo: {stdout:?}");
    // Unknown skill passes through unchanged (echoed as-is).
    let out2 = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(&tmp)
        .arg("--print")
        .arg("--agent-dir").arg(tmp.join("agent"))
        .arg("--session-file").arg(tmp.join("session2.jsonl"))
        .arg("--prompt").arg("/skill:nope")
        .output()
        .expect("running pi-rust");
    assert_eq!(out2.status.code(), Some(0));
    let stdout2 = String::from_utf8_lossy(&out2.stdout).to_string();
    assert_eq!(stdout2, "echo: /skill:nope\n", "{stdout2:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}
