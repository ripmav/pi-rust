//! WP-1102 e2e: the headless project trust gate. Untrusted `<cwd>/.pi/`
//! content has zero behavioral effect (D8: project skills absent); the
//! explicit grants — `--approve`/`-a`, stored `trust.json` decisions
//! (incl. nearest-ancestor), `defaultProjectTrust` — enable project
//! skills. The system prompt is captured by a before_agent_start
//! extension (same pattern as tests/skills.rs).

use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pi-rust-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_skill(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
}

const CAPTURE_EXT: &str = r#"
import { writeFileSync } from "node:fs";

export default function (pi: any) {
  pi.on("before_agent_start", (event: any) => {
    const out = process.env.SKILL_PROMPT_DUMP;
    if (out) writeFileSync(out, String(event.systemPrompt ?? ""));
  });
}
"#;

/// One headless run: agent skill `greet` + project skill `proj` in
/// `<workdir>/.pi/skills`; returns (exit code, captured system prompt,
/// stderr).
fn run(workdir: &Path, agent: &Path, extra: &[&str]) -> (i32, String, String) {
    std::fs::create_dir_all(agent).unwrap();
    std::fs::write(agent.join("ext.ts"), CAPTURE_EXT).unwrap();
    let scen = workdir.join("scenario.json");
    std::fs::write(
        &scen,
        json!({"user_messages": ["hello"], "model_scripts": [[{ "text": "hi" }]]}).to_string(),
    )
    .unwrap();
    let dump = workdir.join("dump.txt");
    let out = Command::new(env!("CARGO_BIN_EXE_pi-rust"))
        .current_dir(workdir)
        .env("SKILL_PROMPT_DUMP", &dump)
        .args([
            "--scenario",
            scen.display().to_string().as_str(),
            "--agent-dir",
            agent.display().to_string().as_str(),
            "--session-file",
            workdir.join("session.jsonl").display().to_string().as_str(),
            "--extension-dir",
            agent.join("ext.ts").display().to_string().as_str(),
        ])
        .args(extra)
        .output()
        .expect("running pi-rust");
    let code = out.status.code().unwrap_or(-1);
    let prompt = std::fs::read_to_string(&dump).unwrap_or_default();
    (
        code,
        prompt,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The standard fixture: an agent skill plus a project skill (`.pi/skills`
/// is a trust-requiring resource by itself).
fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let tmp = tmp(tag);
    let agent = tmp.join("agent");
    write_skill(
        &agent,
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: agent greet\n---\nGreet.",
    );
    write_skill(
        &tmp,
        ".pi/skills/proj/SKILL.md",
        "---\nname: proj\ndescription: project skill\n---\nBody.",
    );
    (tmp, agent)
}

fn has_proj(prompt: &str) -> bool {
    prompt.contains("<name>proj</name>")
}

/// D7: headless (no UI) with the default "ask" → not trusted; project
/// skills absent, agent skills unaffected.
#[test]
fn untrusted_headless_default_ask_project_skills_absent() {
    let (work, agent) = fixture("wp1102a");
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(prompt.contains("<name>greet</name>"), "{prompt}");
    assert!(
        !has_proj(&prompt),
        "untrusted: project skills must not load\n{prompt}"
    );
    let _ = std::fs::remove_dir_all(&work);
}

/// Zero behavioral effect (S3 acceptance): an untrusted run with project
/// files is identical to one without them.
#[test]
fn untrusted_identical_with_or_without_project_files() {
    let (work, agent) = fixture("wp1102b");
    let (code, with_files, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    // Same project minus the .pi tree.
    let work2 = tmp("wp1102b2");
    let (code2, without, stderr2) = run(&work2, &agent, &[]);
    assert_eq!(code2, 0, "{stderr2}");
    assert_eq!(with_files, without, "untrusted project files must be inert");
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&work2);
}

/// D5: --approve / -a grant trust for the run (both forms).
#[test]
fn approve_flag_trusts_project_skills() {
    let (work, agent) = fixture("wp1102c");
    let (code, prompt, stderr) = run(&work, &agent, &["--approve"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        has_proj(&prompt),
        "--approve must load project skills\n{prompt}"
    );
    let (code, prompt, stderr) = run(&work, &agent, &["-a"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(has_proj(&prompt), "-a must load project skills\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);
}

/// Stored decisions (trust.json) are honored headless: true trusts,
/// false does not, nearest-ancestor entries are inherited.
#[test]
fn stored_decisions_headless() {
    // {cwd: true}
    let (work, agent) = fixture("wp1102d");
    let key = std::fs::canonicalize(&work).unwrap();
    std::fs::write(
        agent.join("trust.json"),
        format!("{{\"{}\": true}}", key.display()),
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(has_proj(&prompt), "stored true must trust\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);

    // {cwd: false}
    let (work, agent) = fixture("wp1102e");
    let key = std::fs::canonicalize(&work).unwrap();
    std::fs::write(
        agent.join("trust.json"),
        format!("{{\"{}\": false}}", key.display()),
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!has_proj(&prompt), "stored false must not trust\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);

    // Nearest ancestor: an entry for the PARENT of cwd inherits down.
    let parent = tmp("wp1102f-parent");
    std::fs::create_dir_all(parent.join("proj")).unwrap();
    let work = parent.join("proj");
    let agent = parent.join("agent");
    write_skill(
        &agent,
        "skills/greet/SKILL.md",
        "---\nname: greet\ndescription: agent greet\n---\nGreet.",
    );
    write_skill(
        &work,
        ".pi/skills/proj/SKILL.md",
        "---\nname: proj\ndescription: project skill\n---\nBody.",
    );
    let pkey = std::fs::canonicalize(&parent).unwrap();
    std::fs::write(
        agent.join("trust.json"),
        format!("{{\"{}\": true}}", pkey.display()),
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        has_proj(&prompt),
        "parent entry must be inherited by the child cwd\n{prompt}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// D5: --no-approve / -na deny trust for the run, even over a stored
/// trusted decision (both forms).
#[test]
fn no_approve_overrides_stored_true() {
    let (work, agent) = fixture("wp1102g");
    let key = std::fs::canonicalize(&work).unwrap();
    std::fs::write(
        agent.join("trust.json"),
        format!("{{\"{}\": true}}", key.display()),
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &["--no-approve"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        !has_proj(&prompt),
        "--no-approve must beat stored true\n{prompt}"
    );
    let (code, prompt, stderr) = run(&work, &agent, &["-na"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!has_proj(&prompt), "-na must beat stored true\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);
}

/// D4: defaultProjectTrust always/never from the agent settings.
#[test]
fn default_project_trust_always_never() {
    // "always": trusted without any store entry.
    let (work, agent) = fixture("wp1102h");
    std::fs::write(
        agent.join("settings.json"),
        r#"{"defaultProjectTrust":"always"}"#,
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(has_proj(&prompt), "always must trust\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);

    // "never": untrusted even with a stored true.
    let (work, agent) = fixture("wp1102i");
    std::fs::write(
        agent.join("settings.json"),
        r#"{"defaultProjectTrust":"never"}"#,
    )
    .unwrap();
    let key = std::fs::canonicalize(&work).unwrap();
    std::fs::write(
        agent.join("trust.json"),
        format!("{{\"{}\": true}}", key.display()),
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    // A stored decision wins over "never" (upstream order: step 4 before 5).
    assert!(has_proj(&prompt), "stored decision beats never\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);

    // "never" with no store entry: untrusted.
    let (work, agent) = fixture("wp1102j");
    std::fs::write(
        agent.join("settings.json"),
        r#"{"defaultProjectTrust":"never"}"#,
    )
    .unwrap();
    let (code, prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!has_proj(&prompt), "never must not trust\n{prompt}");
    let _ = std::fs::remove_dir_all(&work);
}

/// D4: an unknown defaultProjectTrust value is a startup error naming the
/// path (exit 1).
#[test]
fn unknown_default_project_trust_errors() {
    let (work, agent) = fixture("wp1102k");
    std::fs::write(
        agent.join("settings.json"),
        r#"{"defaultProjectTrust":"sometimes"}"#,
    )
    .unwrap();
    let (code, _prompt, stderr) = run(&work, &agent, &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("defaultProjectTrust"), "{stderr}");
    assert!(stderr.contains("settings.json"), "{stderr}");
    let _ = std::fs::remove_dir_all(&work);
}
