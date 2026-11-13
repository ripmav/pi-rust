//! Skills (WP-801), ported semantics from upstream `core/skills.ts` @ 085c84e,
//! with recorded v0 deviations (feature doc `m8-ecosystem.md` L2):
//! - discovery: `<agentDir>/skills` + `<cwd>/.pi/skills` only (no `.agents`
//!   ancestor walk, no settings/CLI/extension skill paths)
//! - minimal frontmatter parser: top-level `key: value` scalars only (block
//!   scalars / multiline values fail to parse; upstream uses the yaml lib)
//! - no `.gitignore`/`.ignore`/`.fdignore` filtering, no symlink dedupe,
//!   fixed `failed to parse skill file` message (upstream surfaces the yaml
//!   error text)
use std::path::{Path, PathBuf};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Default)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    disable: bool,
    ok: bool,
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Upstream `extractFrontmatter` verbatim: content must start with `---`,
/// the block ends at the next `\n---`; the body is everything after, trimmed.
fn extract_frontmatter(content: &str) -> (Option<String>, String) {
    let normalized = normalize_newlines(content.strip_prefix('\u{feff}').unwrap_or(content));
    if !normalized.starts_with("---") {
        return (None, normalized.to_string());
    }
    match normalized[3..].find("\n---") {
        Some(rel) => {
            let end = 3 + rel;
            let yaml = normalized[4..end].to_string();
            let body = normalized[end + 4..].trim().to_string();
            (Some(yaml), body)
        }
        None => (None, normalized),
    }
}

fn unquote(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

/// Minimal frontmatter parse (v0): top-level `key: value` scalars for
/// `name` / `description` / `disable-model-invocation`; unknown fields
/// ignored. Any indented line or missing colon = unparseable.
fn parse_frontmatter(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter {
        ok: true,
        ..Default::default()
    };
    for line in yaml.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            fm.ok = false;
            break;
        }
        let Some((key, rest)) = line.split_once(':') else {
            fm.ok = false;
            break;
        };
        let key = key.trim();
        let value = rest.trim();
        match key {
            "name" | "description" => {
                let v = unquote(value);
                match key {
                    "name" => fm.name = Some(v),
                    _ => fm.description = Some(v),
                }
            }
            "disable-model-invocation" => {
                if value != "true" && value != "false" {
                    fm.ok = false;
                    break;
                }
                fm.disable = value == "true";
            }
            _ => {}
        }
    }
    fm
}

/// Upstream `validateName` messages, verbatim.
fn validate_name(name: &str) -> Vec<String> {
    let mut e = Vec::new();
    if name.len() > MAX_NAME_LENGTH {
        e.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        e.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        e.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        e.push("name must not contain consecutive hyphens".to_string());
    }
    e
}

/// Upstream `validateDescription` messages, verbatim.
fn validate_description(d: Option<&str>) -> Vec<String> {
    let mut e = Vec::new();
    match d {
        Some(s) if !s.trim().is_empty() => {
            if s.len() > MAX_DESCRIPTION_LENGTH {
                e.push(format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
                    s.len()
                ));
            }
        }
        _ => e.push("description is required".to_string()),
    }
    e
}

/// Upstream `loadSkillFromFile` semantics: read error / parse error /
/// missing description all skip the skill (declared `SKILL.md` gets a
/// warning for read+parse errors; plain `.md` skips silently without a
/// description). A skill with a valid description but an invalid name
/// still loads (warnings only).
fn load_skill_from_file(path: &Path, warnings: &mut Vec<String>) -> Option<Skill> {
    let is_declared = path.file_name().map(|n| n == "SKILL.md").unwrap_or(false);
    let raw = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(format!(
                "failed to read skill file: {e} ({})",
                path.display()
            ));
            return None;
        }
    };
    let (yaml, _body) = extract_frontmatter(&raw);
    let fm = match yaml {
        Some(y) => {
            let f = parse_frontmatter(&y);
            if !f.ok {
                if is_declared {
                    warnings.push(format!("failed to parse skill file ({})", path.display()));
                }
                return None;
            }
            f
        }
        None => Frontmatter {
            ok: true,
            ..Default::default()
        },
    };
    let description = fm.description.clone();
    let has_description = description
        .as_deref()
        .map(str::trim)
        .is_some_and(|t| !t.is_empty());
    if !is_declared && !has_description {
        return None;
    }
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let parent_name = base_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    for e in validate_description(description.as_deref()) {
        warnings.push(format!("{e} ({})", path.display()));
    }
    // Upstream: `frontmatterName || parentDirName` — an empty string name
    // falls back; a whitespace-only name does not (it then fails validation).
    let name = match &fm.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => parent_name,
    };
    for e in validate_name(&name) {
        warnings.push(format!("{e} ({})", path.display()));
    }
    if !has_description {
        return None;
    }
    Some(Skill {
        name,
        description: description.unwrap(),
        file_path: path.to_path_buf(),
        base_dir,
        disable_model_invocation: fm.disable,
    })
}

/// Upstream `loadSkillsFromDir` rules: a dir containing `SKILL.md` is a
/// skill root (no deeper recursion); otherwise direct root `.md` children
/// (top level only) plus recursion into subdirectories. Dot-dirs and
/// `node_modules` are skipped.
fn load_from_dir_internal(
    dir: &Path,
    include_root_files: bool,
    warnings: &mut Vec<String>,
) -> Vec<Skill> {
    let mut skills = Vec::new();
    if !dir.is_dir() {
        return skills;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return skills;
    };
    let entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    for p in &entries {
        if p.file_name().map(|n| n == "SKILL.md").unwrap_or(false) && p.is_file() {
            if let Some(s) = load_skill_from_file(p, warnings) {
                skills.push(s);
            }
            return skills;
        }
    }
    for p in &entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            skills.extend(load_from_dir_internal(p, false, warnings));
        } else if p.is_file() && include_root_files && name.ends_with(".md") {
            if let Some(s) = load_skill_from_file(p, warnings) {
                skills.push(s);
            }
        }
    }
    skills
}

pub fn load_from_dir(dir: &Path, warnings: &mut Vec<String>) -> Vec<Skill> {
    load_from_dir_internal(dir, true, warnings)
}

/// Upstream `loadSkills` core: agent dir first, then project dir;
/// first-loaded name wins (collisions are warned).
pub fn load(agent_dir: &Path, cwd: &Path, warnings: &mut Vec<String>) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let mut names: std::collections::HashSet<String> = Default::default();
    for dir in [agent_dir.join("skills"), cwd.join(".pi").join("skills")] {
        for s in load_from_dir(&dir, warnings) {
            if !names.insert(s.name.clone()) {
                warnings.push(format!(
                    "name \"{}\" collision ({})",
                    s.name,
                    s.file_path.display()
                ));
                continue;
            }
            out.push(s);
        }
    }
    out
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Upstream `formatSkillsForPrompt` verbatim (pi-rust always exposes the
/// `read` tool, so the reader hint is fixed to `read`). `SKILL.md`
/// locations are absolute paths as discovered.
pub fn format_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "Resolve relative paths against the skill's directory (parent of SKILL.md). Use absolute paths in tool calls.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for s in &visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&s.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&s.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&s.file_path.to_string_lossy())
        ));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

/// Upstream `system-prompt.ts` section wrapping: `<skills>\n{block.trim()}\n</skills>`,
/// sections joined with `\n\n`.
pub fn append_section(base: &str, skills: &[Skill]) -> String {
    let block = format_for_prompt(skills);
    let trimmed = block.trim();
    if trimmed.is_empty() {
        return base.to_string();
    }
    format!("{base}\n\n<skills>\n{trimmed}\n</skills>")
}

/// Upstream `_expandSkillCommand` block, verbatim: the file body with
/// frontmatter stripped, wrapped in a `<skill>` tag; args appended.
pub fn expand_skill(skill: &Skill, args: &str) -> Result<String, String> {
    let raw = std::fs::read_to_string(&skill.file_path).map_err(|e| e.to_string())?;
    let (_yaml, body) = extract_frontmatter(&raw);
    let body = body.trim();
    let block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        skill.name,
        skill.file_path.display(),
        skill.base_dir.display(),
        body
    );
    Ok(if args.is_empty() {
        block
    } else {
        format!("{block}\n\n{args}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-sk-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn skill(d: &Path, rel: &str, content: &str) -> PathBuf {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn extract_frontmatter_split_and_no_fm() {
        assert_eq!(
            extract_frontmatter("---\nname: x\n---\nbody"),
            (Some("name: x".to_string()), "body".to_string())
        );
        assert_eq!(
            extract_frontmatter("no fm here"),
            (None, "no fm here".to_string())
        );
        // Unterminated block: no frontmatter (upstream indexOf -1).
        assert_eq!(
            extract_frontmatter("---\nname: x"),
            (None, "---\nname: x".to_string())
        );
        // CRLF + BOM normalization.
        assert_eq!(
            extract_frontmatter("\u{feff}---\r\nname: x\r\n---\r\nb"),
            (Some("name: x".to_string()), "b".to_string())
        );
    }

    #[test]
    fn frontmatter_scalars_quotes_and_unknown() {
        let fm = parse_frontmatter(
            "name: \"a-b\"\ndescription: 'd 1'\nunknown: [1, 2]\ndisable-model-invocation: true",
        );
        assert!(fm.ok);
        assert_eq!(fm.name.as_deref(), Some("a-b"));
        assert_eq!(fm.description.as_deref(), Some("d 1"));
        assert!(fm.disable);
    }

    #[test]
    fn frontmatter_unparseable_shapes() {
        assert!(!parse_frontmatter("name: x\n  nested: y").ok);
        assert!(!parse_frontmatter("no-colon-line").ok);
        assert!(!parse_frontmatter("disable-model-invocation: maybe").ok);
    }

    #[test]
    fn validate_name_messages_verbatim() {
        assert_eq!(validate_name("ok-name"), Vec::<String>::new());
        let long = "a".repeat(65);
        assert_eq!(
            validate_name(&long),
            vec!["name exceeds 64 characters (65)".to_string()]
        );
        assert_eq!(
            validate_name("Bad_Name"),
            vec!["name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"]
        );
        assert_eq!(
            validate_name("-x"),
            vec!["name must not start or end with a hyphen"]
        );
        assert_eq!(
            validate_name("a--b"),
            vec!["name must not contain consecutive hyphens"]
        );
    }

    #[test]
    fn validate_description_messages_verbatim() {
        assert_eq!(validate_description(None), vec!["description is required"]);
        assert_eq!(
            validate_description(Some("  ")),
            vec!["description is required"]
        );
        let long = "x".repeat(1025);
        assert_eq!(
            validate_description(Some(&long)),
            vec!["description exceeds 1024 characters (1025)"]
        );
        assert_eq!(validate_description(Some("fine")), Vec::<String>::new());
    }

    #[test]
    fn discovery_agent_project_nested_and_root_md() {
        let d = dir();
        let agent = d.join("agent");
        let cwd = d.join("cwd");
        skill(
            &agent,
            "skills/greet/SKILL.md",
            "---\nname: greet\ndescription: agent greet\n---\nbody",
        );
        skill(
            &agent,
            "skills/loose.md",
            "---\ndescription: loose agent md\n---\nbody",
        );
        skill(
            &agent,
            "skills/nested/deep/SKILL.md",
            "---\nname: deep\ndescription: nested\n---\nbody",
        );
        skill(
            &cwd,
            ".pi/skills/proj/SKILL.md",
            "---\nname: proj\ndescription: project skill\n---\nbody",
        );
        let mut w = Vec::new();
        let s = load(&agent, &cwd, &mut w);
        // `loose.md` (root .md, no name field) falls back to the parent dir
        // name "skills" — upstream `parentDirName` behavior. readdir order
        // is filesystem-dependent, so compare as a set.
        let mut names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["deep", "greet", "proj", "skills"],
            "{names:?} warnings: {w:?}"
        );
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn skill_md_root_stops_recursion() {
        let d = dir();
        let agent = d.join("agent");
        skill(
            &agent,
            "skills/root/SKILL.md",
            "---\nname: root\ndescription: root skill\n---\nbody",
        );
        skill(
            &agent,
            "skills/root/sub/inner.md",
            "---\nname: inner\ndescription: hidden\n---\nbody",
        );
        let mut w = Vec::new();
        let s = load_from_dir(&agent.join("skills"), &mut w);
        assert_eq!(s.len(), 1, "SKILL.md root must not recurse: {s:?}");
    }

    #[test]
    fn skips_dotdirs_and_node_modules() {
        let d = dir();
        let agent = d.join("agent");
        skill(
            &agent,
            "skills/.hidden/SKILL.md",
            "---\nname: hidden\ndescription: dot\n---\nbody",
        );
        skill(
            &agent,
            "skills/node_modules/pkg/SKILL.md",
            "---\nname: pkg\ndescription: nm\n---\nbody",
        );
        skill(
            &agent,
            "skills/good/SKILL.md",
            "---\nname: good\ndescription: good\n---\nbody",
        );
        let mut w = Vec::new();
        let s = load_from_dir(&agent.join("skills"), &mut w);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "good");
    }

    #[test]
    fn invalid_skill_skipped_with_warning() {
        let d = dir();
        let agent = d.join("agent");
        // Declared skill without description: skipped + warning.
        skill(
            &agent,
            "skills/nodesc/SKILL.md",
            "---\nname: nodesc\n---\nbody",
        );
        // Plain .md without description: silent skip.
        skill(&agent, "skills/plain.md", "just text");
        // Invalid name chars: skill loads WITH warnings (upstream behavior);
        // the frontmatter name is kept as-is (fallback is only for a missing
        // or empty name).
        skill(
            &agent,
            "skills/bad/SKILL.md",
            "---\nname: Bad_Name\ndescription: has warnings\n---\nbody",
        );
        let mut w = Vec::new();
        let s = load_from_dir(&agent.join("skills"), &mut w);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(
            s[0].name, "Bad_Name",
            "frontmatter name is kept; only warnings are emitted"
        );
        assert!(
            w.iter().any(|m| m.starts_with("description is required")),
            "{w:?}"
        );
        assert!(
            w.iter()
                .any(|m| m.starts_with("name contains invalid characters")),
            "{w:?}"
        );
        assert!(
            !w.iter().any(|m| m.contains("plain.md")),
            "plain .md must be silent: {w:?}"
        );
    }

    #[test]
    fn name_collision_first_wins_with_warning() {
        let d = dir();
        let agent = d.join("agent");
        let cwd = d.join("cwd");
        skill(
            &agent,
            "skills/dup/SKILL.md",
            "---\nname: dup\ndescription: agent one\n---\nbody",
        );
        skill(
            &cwd,
            ".pi/skills/dup2/SKILL.md",
            "---\nname: dup\ndescription: project one\n---\nbody",
        );
        let mut w = Vec::new();
        let s = load(&agent, &cwd, &mut w);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].description, "agent one");
        assert!(
            w.iter().any(|m| m.starts_with("name \"dup\" collision")),
            "{w:?}"
        );
    }

    #[test]
    fn format_for_prompt_golden_matches_upstream() {
        let d = dir();
        let a = Skill {
            name: "alpha".into(),
            description: "A <great> & \"fine\" skill".into(),
            file_path: d.join("alpha/SKILL.md"),
            base_dir: d.join("alpha"),
            disable_model_invocation: false,
        };
        let b = Skill {
            name: "hidden".into(),
            description: "not in the prompt".into(),
            file_path: d.join("hidden/SKILL.md"),
            base_dir: d.join("hidden"),
            disable_model_invocation: true,
        };
        let c = Skill {
            name: "beta".into(),
            description: "second".into(),
            file_path: d.join("beta/SKILL.md"),
            base_dir: d.join("beta"),
            disable_model_invocation: false,
        };
        let expected = format!(
            "\n\nThe following skills provide specialized instructions for specific tasks.\n\
             Use the read tool to load a skill's file when the task matches its description.\n\
             Resolve relative paths against the skill's directory (parent of SKILL.md). Use absolute paths in tool calls.\n\
             \n\
             <available_skills>\n\
             \u{20}\u{20}<skill>\n\
             \u{20}\u{20}\u{20}\u{20}<name>alpha</name>\n\
             \u{20}\u{20}\u{20}\u{20}<description>A &lt;great&gt; &amp; &quot;fine&quot; skill</description>\n\
             \u{20}\u{20}\u{20}\u{20}<location>{}</location>\n\
             \u{20}\u{20}</skill>\n\
             \u{20}\u{20}<skill>\n\
             \u{20}\u{20}\u{20}\u{20}<name>beta</name>\n\
             \u{20}\u{20}\u{20}\u{20}<description>second</description>\n\
             \u{20}\u{20}\u{20}\u{20}<location>{}</location>\n\
             \u{20}\u{20}</skill>\n\
             </available_skills>",
            a.file_path.display(),
            c.file_path.display()
        );
        assert_eq!(format_for_prompt(&[a.clone(), b.clone(), c]), expected);
        // All disabled -> empty.
        assert_eq!(format_for_prompt(&[b]), "");
    }

    #[test]
    fn append_section_golden_and_empty_passthrough() {
        let d = dir();
        let s = Skill {
            name: "s1".into(),
            description: "d1".into(),
            file_path: d.join("s1/SKILL.md"),
            base_dir: d.join("s1"),
            disable_model_invocation: false,
        };
        let expected = format!(
            "BASE\n\n<skills>\nThe following skills provide specialized instructions for specific tasks.\n\
             Use the read tool to load a skill's file when the task matches its description.\n\
             Resolve relative paths against the skill's directory (parent of SKILL.md). Use absolute paths in tool calls.\n\
             \n\
             <available_skills>\n\
             \u{20}\u{20}<skill>\n\
             \u{20}\u{20}\u{20}\u{20}<name>s1</name>\n\
             \u{20}\u{20}\u{20}\u{20}<description>d1</description>\n\
             \u{20}\u{20}\u{20}\u{20}<location>{}</location>\n\
             \u{20}\u{20}</skill>\n\
             </available_skills>\n</skills>",
            s.file_path.display()
        );
        assert_eq!(append_section("BASE", &[s]), expected);
        assert_eq!(append_section("BASE", &[]), "BASE");
    }

    #[test]
    fn expand_skill_golden_with_and_without_args() {
        let d = dir();
        let p = skill(
            &d,
            "greet/SKILL.md",
            "---\nname: greet\ndescription: g\n---\n# Title\n\nDo the thing.",
        );
        let s = load_from_dir(&d, &mut Vec::new()).pop().unwrap();
        let base = format!(
            "<skill name=\"greet\" location=\"{}\">\nReferences are relative to {}.\n\n# Title\n\nDo the thing.\n</skill>",
            p.display(),
            d.join("greet").display()
        );
        assert_eq!(expand_skill(&s, "").unwrap(), base);
        assert_eq!(
            expand_skill(&s, "extra args").unwrap(),
            format!("{base}\n\nextra args")
        );
    }
}
