//! Prompt templates (WP-307), ported semantics from upstream
//! `core/prompt-templates.ts` with recorded v0 deviations:
//! - discovery: `<agentDir>/prompts/*.md` + `<cwd>/.pi/prompts/*.md`
//!   (project wins on name collisions)
//! - name = file stem (frontmatter is NOT parsed in v0 — content is the
//!   raw file)
//! - expansion: `$1`..`$9`, `$@`/`$ARGUMENTS`, `${N:-default}`,
//!   `${ARGUMENTS:-default}` / `${@:-default}`; one pass, no recursion,
//!   no `${@:N:M}` slicing (upstream has it; not needed by v0 templates)
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Template {
    pub name: String,
    pub content: String,
    pub path: PathBuf,
}

/// Discovers prompt templates; project-dir templates shadow agent-dir ones.
pub fn discover(agent_dir: &Path, cwd: &Path) -> Vec<Template> {
    let mut map: std::collections::BTreeMap<String, Template> = std::collections::BTreeMap::new();
    for dir in [agent_dir.join("prompts"), cwd.join(".pi").join("prompts")] {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().is_some_and(|x| x != "md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&p) else {
                continue;
            };
            let name = p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            map.insert(
                name.clone(),
                Template {
                    name,
                    content,
                    path: p,
                },
            );
        }
    }
    map.into_values().collect()
}

/// Bash-style argument parse (upstream `parseCommandArgs` subset): quotes
/// only open at the start of an argument; backslash escapes the quote char.
pub fn parse_args(s: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_quote: Option<char> = None;
    let chars = s.chars().peekable();
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match in_quote {
            Some(q) => {
                if c == '\\' && i + 1 < bytes.len() && bytes[i + 1] == q {
                    current.push(q);
                    i += 1;
                } else if c == q {
                    in_quote = None;
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '"' | '\'' if !has_token => {
                    in_quote = Some(c);
                    has_token = true;
                }
                c if c.is_whitespace() => {
                    if has_token {
                        args.push(std::mem::take(&mut current));
                        has_token = false;
                    }
                }
                _ => {
                    current.push(c);
                    has_token = true;
                }
            },
        }
        i += 1;
    }
    if has_token {
        args.push(current);
    }
    let _ = chars;
    args
}

/// One-pass placeholder substitution (no recursion into substituted text).
pub fn expand(content: &str, args: &[String]) -> String {
    let all = args.join(" ");
    let mut out = String::new();
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' && i + 1 < chars.len() {
            let n = chars[i + 1];
            if n.is_ascii_digit() {
                let idx = (n as u8 - b'0') as usize;
                if args.len() >= idx {
                    out.push_str(&args[idx - 1]);
                    i += 2;
                    continue;
                }
            } else if n == '@' {
                out.push_str(&all);
                i += 2;
                continue;
            } else if n == 'A' && content[i + 1..].starts_with("ARGUMENTS") {
                out.push_str(&all);
                i += 10;
                continue;
            } else if n == '{' {
                if let Some(close) = content[i..].find('}') {
                    let inner = &content[i + 2..i + close];
                    let (target, default) = match inner.split_once(":-") {
                        Some((t, d)) => (t, Some(d.to_string())),
                        None => (inner, None),
                    };
                    let value = match target {
                        "ARGUMENTS" | "@" => {
                            if all.is_empty() {
                                None
                            } else {
                                Some(all.as_str())
                            }
                        }
                        d if d.chars().all(|c| c.is_ascii_digit()) && !d.is_empty() => args
                            .get(d.parse::<usize>().unwrap() - 1)
                            .map(String::as_str),
                        _ => None,
                    };
                    match (value, default) {
                        (Some(v), _) => out.push_str(v),
                        (None, Some(d)) => out.push_str(&d),
                        (None, None) => out.push_str(&content[i..i + close + 1]),
                    }
                    i += close + 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Resolves a template by name and expands it with the given argument string.
pub fn apply(agent_dir: &Path, cwd: &Path, name: &str, arg_string: &str) -> Result<String> {
    let t = discover(agent_dir, cwd)
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown prompt template {name} (looked in <agentDir>/prompts and .pi/prompts)"
            )
        })?;
    Ok(expand(&t.content, &parse_args(arg_string)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-tpl-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn discovers_agent_and_project_with_project_winning() {
        let d = dir();
        let agent = d.join("agent");
        std::fs::create_dir_all(agent.join("prompts")).unwrap();
        std::fs::write(agent.join("prompts/greet.md"), "agent-greet $1").unwrap();
        std::fs::write(agent.join("prompts/only-agent.md"), "A").unwrap();
        let cwd = d.join("cwd");
        std::fs::create_dir_all(cwd.join(".pi/prompts")).unwrap();
        std::fs::write(cwd.join(".pi/prompts/greet.md"), "proj-greet $1").unwrap();
        std::fs::write(cwd.join(".pi/prompts/notes.txt"), "not a template").unwrap();
        let t = discover(&agent, &cwd);
        let names: Vec<&str> = t.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["greet", "only-agent"], "{names:?}");
        let greet = t.iter().find(|x| x.name == "greet").unwrap();
        assert_eq!(greet.content, "proj-greet $1", "project shadows agent");
    }

    #[test]
    fn expands_positional_all_and_defaults() {
        let a = vec!["x".into(), "y".into()];
        assert_eq!(expand("hi $1 $2", &a), "hi x y");
        assert_eq!(expand("all: $@", &a), "all: x y");
        assert_eq!(expand("all: $ARGUMENTS", &a), "all: x y");
        assert_eq!(expand("n: ${2:-none}", &a), "n: y");
        assert_eq!(expand("n: ${9:-none}", &a), "n: none");
        assert_eq!(expand("a: ${ARGUMENTS:-none}", &Vec::new()), "a: none");
        assert_eq!(expand("a: ${ARGUMENTS:-none}", &a), "a: x y");
        // One pass: substituted text is not re-scanned.
        assert_eq!(expand("$1", &["$1".into()]), "$1");
        // $A alone is literal (upstream only knows $@ / $ARGUMENTS).
        assert_eq!(expand("$A", &a), "$A");
    }

    #[test]
    fn parse_args_respects_quotes() {
        assert_eq!(parse_args("a b"), vec!["a", "b"]);
        assert_eq!(parse_args("\"two words\" c"), vec!["two words", "c"]);
        assert_eq!(parse_args("'q' d"), vec!["q", "d"]);
        assert_eq!(parse_args("say \"hi\\\"there\""), vec!["say", "hi\"there"]);
        assert_eq!(parse_args("  spaced  out  "), vec!["spaced", "out"]);
    }

    #[test]
    fn apply_unknown_template_is_typed_error() {
        let d = dir();
        let err = apply(&d, &d, "nope", "").unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }
}
