//! WP-608: slash commands for the TUI editor.
//!
//! Mirrors the pinned upstream interactive mode: the built-in command set
//! (`BUILTIN_SLASH_COMMANDS` from `core/slash-commands.ts` at the pinned
//! commit, plus pi-rust's own `theme` command from WP-607), prompt
//! templates (WP-307) and extension `registerCommand` commands (M1).
//!
//! The menu opens while the first editor line is a `/name` prefix (no
//! space yet); navigation uses the `tui.select.*` bindings. Dispatch is
//! split into local actions (handled in place by the TUI) and bridge
//! actions (returned to the caller, which sends the request over the
//! unchanged NDJSON bridge and renders the result event).

/// Where a command comes from (upstream `SlashCommandSource` + builtin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Template,
    Extension,
}

/// One menu entry / dispatchable command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub source: Source,
}

impl Command {
    fn new(name: &str, description: &str, argument_hint: Option<&str>, source: Source) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            argument_hint: argument_hint.map(str::to_string),
            source,
        }
    }
}

/// The upstream built-in command set at the pinned commit (verbatim
/// names/descriptions/argumentHints) plus pi-rust's `theme` command
/// (WP-607; upstream switches themes via the settings menu, not a
/// slash command).
pub const BUILTIN_SLASH_COMMANDS: &[(&str, &str, Option<&str>)] = &[
    ("settings", "Open settings menu", None),
    (
        "model",
        "Select model (opens selector UI)",
        Some("<provider/model>"),
    ),
    ("tree", "Navigate session tree (switch branches)", None),
    ("thinking", "Set thinking level", Some("<level>")),
    (
        "scoped-models",
        "Enable/disable models for Ctrl+P cycling",
        None,
    ),
    (
        "export",
        "Export session (HTML default, or specify path: .html/.jsonl)",
        None,
    ),
    (
        "import",
        "Import a JSONL session, preserving its ID and working directory",
        None,
    ),
    (
        "share",
        "Share session via a Radius artifact or a GitHub gist",
        None,
    ),
    (
        "bug",
        "Report a bug to the Pi developers",
        Some("<description>"),
    ),
    ("copy", "Copy last agent message to clipboard", None),
    ("name", "Set session display name", Some("<name>")),
    ("session", "Show session info and stats", None),
    ("changelog", "Show changelog entries", None),
    ("hotkeys", "Show all keyboard shortcuts", None),
    (
        "fork",
        "Create a new fork from a previous user message",
        None,
    ),
    (
        "clone",
        "Duplicate the current session at the current position",
        None,
    ),
    (
        "trust",
        "Save project trust decision for future sessions",
        None,
    ),
    (
        "login",
        "Configure provider authentication",
        Some("<provider>"),
    ),
    ("logout", "Remove provider authentication", None),
    ("new", "Start a new session", None),
    ("compact", "Manually compact the session context", None),
    ("resume", "Resume another session", None),
    (
        "reload",
        "Reload keybindings, extensions, skills, prompts, themes, and context files",
        None,
    ),
    ("quit", "Quit pi-rust", None),
    // pi-rust addition (WP-607): runtime theme switch.
    ("theme", "Switch theme", Some("<name>")),
];

/// The full command list for the menu, in the upstream order: built-ins,
/// then prompt templates, then extension commands. Upstream rule: an
/// extension command that conflicts with a built-in name is skipped in
/// the autocomplete (the built-in wins).
pub fn collect_menu(
    templates: &[crate::templates::Template],
    extensions: &[Command],
) -> Vec<Command> {
    let mut out: Vec<Command> = BUILTIN_SLASH_COMMANDS
        .iter()
        .map(|(n, d, h)| Command::new(n, d, *h, Source::Builtin))
        .collect();
    out.extend(templates.iter().map(|t| {
        Command::new(
            &t.name,
            &format!(
                "Prompt template: {}",
                t.path.file_name().unwrap_or_default().to_string_lossy()
            ),
            None,
            Source::Template,
        )
    }));
    let existing: Vec<String> = out.iter().map(|t| t.name.clone()).collect();
    out.extend(
        extensions
            .iter()
            .filter(|c| !existing.contains(&c.name))
            .cloned(),
    );
    out
}

/// Menu state for the current input: `Some(items)` while the first line
/// is a `/name` prefix (no space yet) and at least one command matches;
/// `None` when the menu is closed. An empty match set closes the menu
/// (upstream shows no candidates, the text is left as typed).
pub fn menu_items(all: &[Command], input: &str) -> Option<Vec<Command>> {
    let first_line = input.lines().next().unwrap_or("");
    let rest = first_line.strip_prefix('/')?;
    if rest.is_empty() {
        return (!all.is_empty()).then(|| all.to_vec());
    }
    if rest.contains(' ') {
        return None; // past the name: argument territory (v0: no arg completion)
    }
    let matched = all
        .iter()
        .filter(|c| c.name.starts_with(rest))
        .cloned()
        .collect::<Vec<_>>();
    (!matched.is_empty()).then_some(matched)
}

/// Split a submitted `/name args...` line into (name, args).
pub fn parse_invocation(text: &str) -> Option<(&str, &str)> {
    let body = text.strip_prefix('/')?;
    match body.find(char::is_whitespace) {
        Some(i) => Some((&body[..i], body[i..].trim())),
        None => Some((body, "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exts() -> Vec<Command> {
        vec![
            Command::new("deploy", "Deploy the app", None, Source::Extension),
            // Conflicts with the built-in "model": must be skipped.
            Command::new(
                "model",
                "conflicting extension command",
                None,
                Source::Extension,
            ),
        ]
    }

    #[test]
    fn builtin_set_matches_upstream_plus_theme() {
        assert_eq!(BUILTIN_SLASH_COMMANDS.len(), 25, "24 upstream + theme");
        for (name, _, _) in BUILTIN_SLASH_COMMANDS {
            assert!(
                [
                    "settings",
                    "model",
                    "tree",
                    "thinking",
                    "scoped-models",
                    "export",
                    "import",
                    "share",
                    "bug",
                    "copy",
                    "name",
                    "session",
                    "changelog",
                    "hotkeys",
                    "fork",
                    "clone",
                    "trust",
                    "login",
                    "logout",
                    "new",
                    "compact",
                    "resume",
                    "reload",
                    "quit",
                    "theme"
                ]
                .contains(name),
                "unexpected builtin: {name}"
            );
        }
        assert_eq!(BUILTIN_SLASH_COMMANDS[1].0, "model");
        assert_eq!(BUILTIN_SLASH_COMMANDS[1].2, Some("<provider/model>"));
    }

    #[test]
    fn menu_order_and_extension_conflict_skip() {
        use crate::templates::Template;
        use std::path::PathBuf;
        let templates = vec![Template {
            name: "fix".into(),
            content: "fix $1".into(),
            path: PathBuf::from("prompts/fix.md"),
        }];
        let menu = collect_menu(&templates, &exts());
        let names: Vec<&str> = menu.iter().map(|c| c.name.as_str()).collect();
        // Builtins first, template after them, extension last.
        assert_eq!(names[0], "settings");
        assert_eq!(names[24], "theme");
        assert_eq!(names[25], "fix");
        assert_eq!(names[26], "deploy");
        assert_eq!(names.len(), 27, "conflicting extension `model` is skipped");
        assert!(!names.iter().skip(25).any(|n| *n == "model"));
    }

    #[test]
    fn menu_open_states() {
        let menu = collect_menu(&[], &exts());
        // Bare "/" opens with everything.
        assert!(menu_items(&menu, "/").is_some());
        assert_eq!(menu_items(&menu, "/").unwrap().len(), menu.len());
        // A prefix filters.
        let m = menu_items(&menu, "/mo").unwrap();
        assert_eq!(
            m.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["model"]
        );
        // No match closes the menu.
        assert!(menu_items(&menu, "/zzz").is_none());
        // A space closes the menu (argument territory).
        assert!(menu_items(&menu, "/model ").is_none());
        // Non-command input never opens it.
        assert!(menu_items(&menu, "hello").is_none());
        assert!(menu_items(&menu, "").is_none());
        // Multi-line: only the first line matters.
        assert!(menu_items(&menu, "/mo\nsecond").is_some());
        assert!(menu_items(&menu, "hi\n/mo").is_none());
    }

    #[test]
    fn parse_invocation_splits_name_and_args() {
        assert_eq!(parse_invocation("/theme"), Some(("theme", "")));
        assert_eq!(parse_invocation("/theme light"), Some(("theme", "light")));
        assert_eq!(
            parse_invocation("/theme   spaced  out  "),
            Some(("theme", "spaced  out"))
        );
        assert_eq!(parse_invocation("/no-space"), Some(("no-space", "")));
        assert_eq!(parse_invocation("plain text"), None);
    }
}
