//! WP-606: upstream keybinding schema, defaults, and user configuration.
//!
//! The table is the pinned-upstream `TUI_KEYBINDINGS` + `APP_KEYBINDINGS`
//! merge (linux, non-WSL platform values) — the "golden" defaults. User
//! bindings come from `<agentDir>/keybindings.json` (upstream path), with
//! the upstream legacy-name migration. Conflict detection follows upstream:
//! only *user* bindings can conflict; default collisions (e.g. `ctrl+p` on
//! several picker actions) are not flagged.
//!
//! v0 wiring: only the actions the TUI implements are checked in the key
//! path (editor/input/select/app core); the rest resolve and are
//! configurable now and get wired by their feature WPs (pickers: WP-609).
use std::collections::BTreeMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Parsed key identifier: base key + modifier flags (upstream `parseKeyId`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKey {
    pub base: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub super_: bool,
}

const MODIFIERS: [&str; 4] = ["ctrl", "shift", "alt", "super"];

const SPECIAL_KEYS: &[&str] = &[
    "escape",
    "esc",
    "enter",
    "return",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "clear",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

const SYMBOL_KEYS: &[char] = &[
    '`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '!', '@', '#', '$', '%', '^', '&',
    '*', '(', ')', '_', '+', '|', '~', '{', '}', ':', '<', '>', '?',
];

/// Upstream `parseKeyId` grammar: modifiers stripped from the front in any
/// order (each at most once); the remaining base must not contain `+`
/// (a literal `+` base is allowed, e.g. `ctrl++`).
pub fn parse_key_id(key_id: &str) -> Option<ParsedKey> {
    let mut key = key_id.to_lowercase();
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut super_ = false;
    loop {
        let mut matched = false;
        for m in MODIFIERS {
            if let Some(rest) = key.strip_prefix(&format!("{m}+")) {
                let flag = match m {
                    "ctrl" => &mut ctrl,
                    "shift" => &mut shift,
                    "alt" => &mut alt,
                    _ => &mut super_,
                };
                if *flag {
                    return None; // duplicate modifier
                }
                *flag = true;
                key = rest.to_string();
                matched = true;
                break;
            }
        }
        if !matched {
            break;
        }
    }
    if key.is_empty() || (key.contains('+') && key != "+") {
        return None;
    }
    let c = key.chars().next().unwrap_or('\0');
    let valid_base = SPECIAL_KEYS.contains(&key.as_str())
        || (key.len() == 1 && c.is_ascii_lowercase())
        || (key.len() == 1 && c.is_ascii_digit())
        || key == "+"
        || (key.len() == 1 && SYMBOL_KEYS.contains(&c));
    if !valid_base {
        return None;
    }
    Some(ParsedKey {
        base: key,
        ctrl,
        shift,
        alt,
        super_,
    })
}

/// The pinned-upstream golden table (linux, non-WSL): `TUI_KEYBINDINGS`
/// (47) + `APP_KEYBINDINGS` (43) with the platform overrides applied.
pub const KEYBINDINGS: &[(&str, &[&str], &str)] = &[
    // ---- editor (upstream packages/tui/src/keybindings.ts) ----
    ("tui.editor.cursorUp", &["up"], "Move cursor up"),
    ("tui.editor.cursorDown", &["down"], "Move cursor down"),
    (
        "tui.editor.historyPrevious",
        &[],
        "Select previous prompt history entry",
    ),
    (
        "tui.editor.historyNext",
        &[],
        "Select next prompt history entry",
    ),
    (
        "tui.editor.cursorLeft",
        &["left", "ctrl+b"],
        "Move cursor left",
    ),
    (
        "tui.editor.cursorRight",
        &["right", "ctrl+f"],
        "Move cursor right",
    ),
    (
        "tui.editor.cursorWordLeft",
        &["alt+left", "ctrl+left", "alt+b"],
        "Move cursor word left",
    ),
    (
        "tui.editor.cursorWordRight",
        &["alt+right", "ctrl+right", "alt+f"],
        "Move cursor word right",
    ),
    (
        "tui.editor.cursorLineStart",
        &["home", "ctrl+a"],
        "Move to line start",
    ),
    (
        "tui.editor.cursorLineEnd",
        &["end", "ctrl+e"],
        "Move to line end",
    ),
    (
        "tui.editor.jumpForward",
        &["ctrl+]"],
        "Jump forward to character",
    ),
    (
        "tui.editor.jumpBackward",
        &["ctrl+alt+]"],
        "Jump backward to character",
    ),
    ("tui.editor.pageUp", &["pageUp", "ctrl+pageUp"], "Page up"),
    (
        "tui.editor.pageDown",
        &["pageDown", "ctrl+pageDown"],
        "Page down",
    ),
    (
        "tui.editor.deleteCharBackward",
        &["backspace"],
        "Delete character backward",
    ),
    (
        "tui.editor.deleteCharForward",
        &["delete", "ctrl+d"],
        "Delete character forward",
    ),
    (
        "tui.editor.deleteWordBackward",
        &["ctrl+w", "alt+backspace"],
        "Delete word backward",
    ),
    (
        "tui.editor.deleteWordForward",
        &["alt+d", "alt+delete"],
        "Delete word forward",
    ),
    (
        "tui.editor.deleteToLineStart",
        &["ctrl+u"],
        "Delete to line start",
    ),
    (
        "tui.editor.deleteToLineEnd",
        &["ctrl+k"],
        "Delete to line end",
    ),
    ("tui.editor.yank", &["ctrl+y"], "Yank"),
    ("tui.editor.yankPop", &["alt+y"], "Yank pop"),
    ("tui.editor.undo", &["ctrl+-"], "Undo"),
    // ---- input ----
    (
        "tui.input.newLine",
        &["shift+enter", "ctrl+j"],
        "Insert newline",
    ),
    ("tui.input.submit", &["enter"], "Submit input"),
    ("tui.input.tab", &["tab"], "Tab / autocomplete"),
    ("tui.input.copy", &["ctrl+c"], "Copy selection"),
    // ---- selection ----
    ("tui.select.up", &["up"], "Move selection up"),
    ("tui.select.down", &["down"], "Move selection down"),
    ("tui.select.pageUp", &["pageUp"], "Selection page up"),
    ("tui.select.pageDown", &["pageDown"], "Selection page down"),
    ("tui.select.confirm", &["enter"], "Confirm selection"),
    (
        "tui.select.cancel",
        &["escape", "ctrl+c"],
        "Cancel selection",
    ),
    // ---- alternate-screen viewport ----
    (
        "tui.altScreen.pageUp",
        &["pageUp"],
        "Scroll viewport up one page",
    ),
    (
        "tui.altScreen.pageDown",
        &["pageDown"],
        "Scroll viewport down one page",
    ),
    (
        "tui.altScreen.halfPageUp",
        &[],
        "Scroll viewport up half a page",
    ),
    (
        "tui.altScreen.halfPageDown",
        &[],
        "Scroll viewport down half a page",
    ),
    ("tui.altScreen.lineUp", &[], "Scroll viewport up one line"),
    (
        "tui.altScreen.lineDown",
        &[],
        "Scroll viewport down one line",
    ),
    (
        "tui.altScreen.previousPrompt",
        &["ctrl+shift+up", "ctrl+up"],
        "Jump to previous semantic prompt",
    ),
    (
        "tui.altScreen.nextPrompt",
        &["ctrl+shift+down", "ctrl+down"],
        "Jump to next semantic prompt",
    ),
    (
        "tui.altScreen.search",
        &["ctrl+shift+f"],
        "Search the primary scroll view",
    ),
    (
        "tui.altScreen.searchNext",
        &["enter", "ctrl+g"],
        "Select the next search match",
    ),
    (
        "tui.altScreen.searchPrevious",
        &["shift+enter", "ctrl+shift+g"],
        "Select the previous search match",
    ),
    (
        "tui.altScreen.searchClose",
        &["escape"],
        "Close transcript search",
    ),
    (
        "tui.altScreen.top",
        &["ctrl+home"],
        "Scroll viewport to top",
    ),
    (
        "tui.altScreen.bottom",
        &["ctrl+end"],
        "Scroll viewport to bottom",
    ),
    // ---- app (upstream packages/coding-agent/src/core/keybindings.ts, linux) ----
    ("app.interrupt", &["escape"], "Cancel or abort"),
    ("app.clear", &["ctrl+c"], "Clear editor"),
    ("app.exit", &["ctrl+d"], "Exit when editor is empty"),
    ("app.suspend", &["ctrl+z"], "Suspend to background"),
    ("app.thinking.cycle", &["shift+tab"], "Cycle thinking level"),
    ("app.thinking.save", &["ctrl+s"], "Save thinking level"),
    ("app.model.cycleForward", &["ctrl+p"], "Cycle to next model"),
    (
        "app.model.cycleBackward",
        &["shift+ctrl+p"],
        "Cycle to previous model",
    ),
    ("app.model.select", &["ctrl+l"], "Open model selector"),
    ("app.tools.expand", &["ctrl+o"], "Toggle tool output"),
    ("app.thinking.toggle", &["ctrl+t"], "Toggle thinking blocks"),
    (
        "app.session.toggleNamedFilter",
        &["ctrl+n"],
        "Toggle named session filter",
    ),
    ("app.editor.external", &["ctrl+g"], "Open external editor"),
    (
        "app.message.copy",
        &["ctrl+x"],
        "Copy selection or last assistant message",
    ),
    (
        "app.message.followUp",
        &["alt+enter"],
        "Queue follow-up message",
    ),
    (
        "app.message.dequeue",
        &["alt+up"],
        "Restore queued messages",
    ),
    (
        "app.clipboard.pasteImage",
        &["ctrl+v"],
        "Paste files on macOS, images, or text from clipboard",
    ),
    ("app.session.new", &[], "Start a new session"),
    ("app.session.tree", &[], "Open session tree"),
    ("app.session.fork", &[], "Fork current session"),
    ("app.session.resume", &[], "Resume last session"),
    (
        "app.tree.foldOrUp",
        &["ctrl+left", "alt+left"],
        "Fold tree branch or move up",
    ),
    (
        "app.tree.unfoldOrDown",
        &["ctrl+right", "alt+right"],
        "Unfold tree branch or move down",
    ),
    ("app.tree.editLabel", &["shift+l"], "Edit tree label"),
    (
        "app.tree.toggleLabelTimestamp",
        &["shift+t"],
        "Toggle tree label timestamps",
    ),
    (
        "app.session.togglePath",
        &["ctrl+p"],
        "Toggle session path display",
    ),
    (
        "app.session.toggleSort",
        &["ctrl+s"],
        "Toggle session sort mode",
    ),
    ("app.session.rename", &["ctrl+r"], "Rename session"),
    ("app.session.delete", &["ctrl+d"], "Delete session"),
    (
        "app.session.deleteNoninvasive",
        &["ctrl+backspace"],
        "Delete session when query is empty",
    ),
    ("app.models.save", &["ctrl+s"], "Save model selection"),
    ("app.models.enableAll", &["ctrl+a"], "Enable all models"),
    ("app.models.clearAll", &["ctrl+x"], "Clear all models"),
    (
        "app.models.toggleProvider",
        &["ctrl+p"],
        "Toggle all models for provider",
    ),
    (
        "app.models.reorderUp",
        &["alt+up"],
        "Move model up in order",
    ),
    (
        "app.models.reorderDown",
        &["alt+down"],
        "Move model down in order",
    ),
    (
        "app.tree.filter.default",
        &["ctrl+d"],
        "Tree filter: default view",
    ),
    (
        "app.tree.filter.noTools",
        &["ctrl+t"],
        "Tree filter: hide tool results",
    ),
    (
        "app.tree.filter.userOnly",
        &["ctrl+u"],
        "Tree filter: user messages only",
    ),
    (
        "app.tree.filter.labeledOnly",
        &["ctrl+l"],
        "Tree filter: labeled entries only",
    ),
    (
        "app.tree.filter.all",
        &["ctrl+a"],
        "Tree filter: show all entries",
    ),
    (
        "app.tree.filter.cycleForward",
        &["ctrl+o"],
        "Tree filter: cycle forward",
    ),
    (
        "app.tree.filter.cycleBackward",
        &["shift+ctrl+o"],
        "Tree filter: cycle backward",
    ),
];

/// Upstream `KEYBINDING_NAME_MIGRATIONS` (legacy flat names -> namespaced).
pub const LEGACY_NAME_MIGRATIONS: &[(&str, &str)] = &[
    ("cursorUp", "tui.editor.cursorUp"),
    ("cursorDown", "tui.editor.cursorDown"),
    ("cursorLeft", "tui.editor.cursorLeft"),
    ("cursorRight", "tui.editor.cursorRight"),
    ("cursorWordLeft", "tui.editor.cursorWordLeft"),
    ("cursorWordRight", "tui.editor.cursorWordRight"),
    ("cursorLineStart", "tui.editor.cursorLineStart"),
    ("cursorLineEnd", "tui.editor.cursorLineEnd"),
    ("jumpForward", "tui.editor.jumpForward"),
    ("jumpBackward", "tui.editor.jumpBackward"),
    ("pageUp", "tui.editor.pageUp"),
    ("pageDown", "tui.editor.pageDown"),
    ("deleteCharBackward", "tui.editor.deleteCharBackward"),
    ("deleteCharForward", "tui.editor.deleteCharForward"),
    ("deleteWordBackward", "tui.editor.deleteWordBackward"),
    ("deleteWordForward", "tui.editor.deleteWordForward"),
    ("deleteToLineStart", "tui.editor.deleteToLineStart"),
    ("deleteToLineEnd", "tui.editor.deleteToLineEnd"),
    ("yank", "tui.editor.yank"),
    ("yankPop", "tui.editor.yankPop"),
    ("undo", "tui.editor.undo"),
    ("newLine", "tui.input.newLine"),
    ("submit", "tui.input.submit"),
    ("tab", "tui.input.tab"),
    ("copy", "tui.input.copy"),
    ("selectUp", "tui.select.up"),
    ("selectDown", "tui.select.down"),
    ("selectPageUp", "tui.select.pageUp"),
    ("selectPageDown", "tui.select.pageDown"),
    ("selectConfirm", "tui.select.confirm"),
    ("selectCancel", "tui.select.cancel"),
    ("interrupt", "app.interrupt"),
    ("clear", "app.clear"),
    ("exit", "app.exit"),
    ("suspend", "app.suspend"),
    ("cycleThinkingLevel", "app.thinking.cycle"),
    ("cycleModelForward", "app.model.cycleForward"),
    ("cycleModelBackward", "app.model.cycleBackward"),
    ("selectModel", "app.model.select"),
    ("expandTools", "app.tools.expand"),
    ("toggleThinking", "app.thinking.toggle"),
    ("toggleSessionNamedFilter", "app.session.toggleNamedFilter"),
    ("externalEditor", "app.editor.external"),
    ("followUp", "app.message.followUp"),
    ("dequeue", "app.message.dequeue"),
    ("pasteImage", "app.clipboard.pasteImage"),
    ("newSession", "app.session.new"),
    ("tree", "app.session.tree"),
    ("fork", "app.session.fork"),
    ("resume", "app.session.resume"),
    ("treeFoldOrUp", "app.tree.foldOrUp"),
    ("treeUnfoldOrDown", "app.tree.unfoldOrDown"),
    ("treeEditLabel", "app.tree.editLabel"),
    ("treeToggleLabelTimestamp", "app.tree.toggleLabelTimestamp"),
    ("toggleSessionPath", "app.session.togglePath"),
    ("toggleSessionSort", "app.session.toggleSort"),
    ("renameSession", "app.session.rename"),
    ("deleteSession", "app.session.delete"),
    ("deleteSessionNoninvasive", "app.session.deleteNoninvasive"),
];

/// User keybindings: action -> keys (an empty list unbinds).
pub type KeybindingsConfig = BTreeMap<String, Vec<String>>;

pub fn valid_actions() -> Vec<String> {
    KEYBINDINGS.iter().map(|(a, _, _)| a.to_string()).collect()
}

/// Upstream `KeybindingsManager` semantics: user bindings replace the
/// defaults of the bound action (an empty array unbinds it); only user
/// bindings are checked for conflicts; invalid actions or keys are errors
/// (unknown actions list the valid ones).
#[derive(Debug)]
pub struct KeybindingManager {
    /// action -> resolved keys (user keys or the golden defaults).
    resolved: BTreeMap<String, Vec<String>>,
    /// (key, [actions claiming it]) — user claims only (upstream); read by the in-crate tests.
    #[allow(dead_code)]
    conflicts: Vec<(String, Vec<String>)>,
}

impl Default for KeybindingManager {
    fn default() -> Self {
        Self::new(&BTreeMap::new()).expect("golden defaults must be valid")
    }
}

impl KeybindingManager {
    /// Build from user bindings (already migrated + raw). Errors name the
    /// offending entry and list the valid actions (acceptance: invalid
    /// binding -> error listing valid actions).
    pub fn new(user: &KeybindingsConfig) -> Result<Self, String> {
        let mut resolved: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (action, defaults, _) in KEYBINDINGS {
            resolved.insert(
                action.to_string(),
                defaults.iter().map(|s| s.to_string()).collect(),
            );
        }
        // Validate + apply user bindings (legacy names migrate here,
        // idempotently — upstream migrates at load, both paths converge).
        let mut user_claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (action, keys) in user {
            let action = LEGACY_NAME_MIGRATIONS
                .iter()
                .find(|(from, _)| *from == action)
                .map(|(_, to)| to.to_string())
                .unwrap_or_else(|| action.clone());
            if !resolved.contains_key(action.as_str()) {
                let valid = valid_actions().join(", ");
                return Err(format!(
                    "unknown keybinding action {action:?} (valid actions: {valid})"
                ));
            }
            let mut norm: Vec<String> = Vec::new();
            for k in keys {
                match parse_key_id(k) {
                    Some(_) => {
                        if !norm.contains(k) {
                            norm.push(k.clone());
                        }
                    }
                    None => {
                        return Err(format!(
                            "invalid key {k:?} for action {action:?} (expected e.g. \"enter\", \"ctrl+w\", \"shift+ctrl+p\")"
                        ));
                    }
                }
            }
            for k in &norm {
                user_claims
                    .entry(k.clone())
                    .or_default()
                    .push(action.clone());
            }
            resolved.insert(action.clone(), norm);
        }
        let conflicts = user_claims
            .into_iter()
            .filter(|(_, claimants)| claimants.len() > 1)
            .map(|(key, mut claimants)| {
                claimants.sort();
                (key, claimants)
            })
            .collect();
        Ok(Self {
            resolved,
            conflicts,
        })
    }

    /// Load `<agentDir>/keybindings.json` (upstream path). A missing file
    /// yields the defaults; a malformed file or an invalid entry is an
    /// error (the caller logs it and may continue with defaults).
    pub fn load(agent_dir: &Path) -> Result<Self, String> {
        let path = agent_dir.join("keybindings.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("reading {path:?}: {e}"))?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("parsing {path:?}: {e}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| format!("{path:?} must be a JSON object of action -> key(s)"))?;
        let mut user: KeybindingsConfig = BTreeMap::new();
        for (key, val) in obj {
            if key == "$schema" {
                continue;
            }
            let keys: Vec<String> = match val {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                other => {
                    return Err(format!(
                        "invalid binding for {key:?}: expected a key string or an array of key strings, got {other}"
                    ))
                }
            };
            user.insert(key.clone(), keys);
        }
        Self::new(&user)
    }

    /// The resolved keys of an action (user override or golden default).
    pub fn get_keys(&self, action: &str) -> Vec<String> {
        self.resolved.get(action).cloned().unwrap_or_default()
    }

    /// Upstream conflict report: user bindings only; read by the in-crate tests.
    #[allow(dead_code)]
    pub fn conflicts(&self) -> &[(String, Vec<String>)] {
        &self.conflicts
    }

    /// Does the crossterm key event match the action (any of its resolved
    /// keys)? Mirrors upstream `matchesKey` semantics on the parsed
    /// (KeyCode, KeyModifiers) instead of raw escape data: exact modifier
    /// state, shift+letter via the uppercase char or the SHIFT flag.
    pub fn matches(&self, ev: &KeyEvent, action: &str) -> bool {
        self.get_keys(action).iter().any(|k| matches_event(ev, k))
    }
}

/// `matchesKey` on a crossterm event (see `KeybindingManager::matches`).
pub fn matches_event(ev: &KeyEvent, key_id: &str) -> bool {
    let Some(pk) = parse_key_id(key_id) else {
        return false;
    };
    let mods = ev.modifiers;
    let want_ctrl = mods.contains(KeyModifiers::CONTROL);
    let want_shift = mods.contains(KeyModifiers::SHIFT);
    let want_alt = mods.contains(KeyModifiers::ALT);
    let want_super = mods.contains(KeyModifiers::SUPER);
    // Exact modifier state (upstream: the modifier bits must match).
    if want_ctrl != pk.ctrl
        || want_shift != pk.shift
        || want_alt != pk.alt
        || want_super != pk.super_
    {
        return false;
    }
    match ev.code {
        KeyCode::Char(c) => {
            let lower = c.to_ascii_lowercase();
            match pk.base.as_str() {
                "space" => c == ' ' && !pk.shift,
                "escape" | "esc" => false, // Esc is a dedicated code, not a char
                base if base.len() == 1 && base.chars().next().unwrap().is_ascii_lowercase() => {
                    // Letter: shift+letter arrives as the uppercase char
                    // (legacy terminals) or lowercase + SHIFT (Kitty).
                    let event_shift = want_shift || c.is_ascii_uppercase();
                    event_shift == pk.shift && lower == base.chars().next().unwrap()
                }
                base if base.len() == 1 && base.chars().next().unwrap().is_ascii_digit() => {
                    // Digits: shift is cosmetic (same char); accept either.
                    c.is_ascii_digit() && lower == base.chars().next().unwrap()
                }
                base if base.len() == 1 => {
                    // Symbol: exact char; shift state must match (shifted
                    // symbols like '=' carry the SHIFT flag in crossterm).
                    c.to_string() == base && want_shift == pk.shift
                }
                "+" => c == '+' && want_shift == pk.shift,
                _ => false,
            }
        }
        KeyCode::Enter => pk.base == "enter" || pk.base == "return",
        KeyCode::Esc => (pk.base == "escape" || pk.base == "esc") && !pk.shift,
        KeyCode::Tab => pk.base == "tab",
        KeyCode::Backspace => pk.base == "backspace",
        KeyCode::Delete => pk.base == "delete",
        KeyCode::Insert => pk.base == "insert",
        KeyCode::Home => pk.base == "home",
        KeyCode::End => pk.base == "end",
        KeyCode::PageUp => pk.base == "pageup",
        KeyCode::PageDown => pk.base == "pagedown",
        KeyCode::Up => pk.base == "up",
        KeyCode::Down => pk.base == "down",
        KeyCode::Left => pk.base == "left",
        KeyCode::Right => pk.base == "right",
        KeyCode::F(n) if (1..=12).contains(&n) => pk.base == format!("f{n}"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn key_mod(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }
    fn key_code(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn config(pairs: &[(&str, Vec<String>)]) -> KeybindingsConfig {
        pairs
            .iter()
            .map(|(a, k)| (a.to_string(), k.clone()))
            .collect()
    }

    #[test]
    fn defaults_match_upstream_golden() {
        // Spot-checks of the pinned upstream defaults (linux, non-WSL).
        let m = KeybindingManager::default();
        assert_eq!(m.get_keys("tui.input.submit"), vec!["enter"]);
        assert_eq!(
            m.get_keys("tui.input.newLine"),
            vec!["shift+enter", "ctrl+j"]
        );
        assert_eq!(
            m.get_keys("tui.editor.deleteWordBackward"),
            vec!["ctrl+w", "alt+backspace"]
        );
        assert_eq!(
            m.get_keys("tui.editor.deleteWordForward"),
            vec!["alt+d", "alt+delete"]
        );
        assert_eq!(m.get_keys("tui.editor.cursorLeft"), vec!["left", "ctrl+b"]);
        assert_eq!(m.get_keys("app.exit"), vec!["ctrl+d"]);
        assert_eq!(m.get_keys("app.clear"), vec!["ctrl+c"]);
        assert_eq!(m.get_keys("tui.editor.undo"), vec!["ctrl+-"]);
        assert_eq!(
            m.get_keys("tui.altScreen.previousPrompt"),
            vec!["ctrl+shift+up", "ctrl+up"]
        );
        assert_eq!(m.get_keys("app.model.cycleBackward"), vec!["shift+ctrl+p"]);
        assert_eq!(m.get_keys("app.message.followUp"), vec!["alt+enter"]);
        assert_eq!(
            m.get_keys("app.tree.foldOrUp"),
            vec!["ctrl+left", "alt+left"]
        );
        assert!(m.get_keys("tui.editor.historyPrevious").is_empty());
        assert_eq!(KEYBINDINGS.len(), 90, "TUI (47) + APP (43) actions");
        // Every default must parse.
        for (action, keys, _) in KEYBINDINGS {
            for k in *keys {
                assert!(parse_key_id(k).is_some(), "{action}: {k}");
            }
        }
    }

    #[test]
    fn parse_key_id_grammar() {
        // Modifiers in any order, each at most once.
        assert!(parse_key_id("shift+ctrl+p").is_some());
        assert!(parse_key_id("ctrl+shift+p").is_some());
        let p = parse_key_id("ctrl+shift+p").unwrap();
        assert_eq!(
            (p.base.clone(), p.ctrl, p.shift),
            ("p".to_string(), true, true)
        );
        assert!(parse_key_id("ctrl+ctrl+p").is_none(), "duplicate modifier");
        assert!(parse_key_id("ctrl+").is_none(), "missing base");
        assert!(parse_key_id("ctrl++").is_some(), "literal + base");
        assert!(parse_key_id("f1").is_some());
        assert!(parse_key_id("F1").is_some(), "case-insensitive");
        assert!(parse_key_id("PAGEUP").is_some());
        assert_eq!(parse_key_id("pageup").unwrap().base, "pageup");
        assert!(parse_key_id("ctrl+ctrl").is_none());
        assert!(parse_key_id("hyper+x").is_none(), "unknown modifier");
        assert!(parse_key_id("ctrl+q+").is_none(), "trailing plus");
    }

    #[test]
    fn matches_semantics_on_crossterm_events() {
        let m = KeybindingManager::default();
        // Letters: shift+letter vs letter.
        // Plain 'a' is not ctrl+a; shift+a is not plain 'a' either.
        assert!(!m.matches(&key('a'), "tui.editor.cursorLineStart"));
        assert!(!m.matches(
            &key_mod('A', KeyModifiers::SHIFT),
            "tui.editor.cursorLineStart"
        ));
        assert!(m.matches(
            &key_mod('a', KeyModifiers::CONTROL),
            "tui.editor.cursorLineStart"
        ));
        assert!(
            !m.matches(&key('b'), "tui.editor.cursorLeft"),
            "plain b is not ctrl+b"
        );
        assert!(m.matches(
            &key_mod('b', KeyModifiers::CONTROL),
            "tui.editor.cursorLeft"
        ));
        assert!(m.matches(
            &key_code(KeyCode::Left, KeyModifiers::NONE),
            "tui.editor.cursorLeft"
        ));
        // Special keys + modifier exactness.
        assert!(m.matches(
            &key_code(KeyCode::Up, KeyModifiers::CONTROL),
            "tui.altScreen.previousPrompt"
        ));
        assert!(!m.matches(
            &key_code(KeyCode::Up, KeyModifiers::NONE),
            "tui.altScreen.previousPrompt"
        ));
        assert!(m.matches(
            &key_code(KeyCode::Enter, KeyModifiers::SHIFT),
            "tui.input.newLine"
        ));
        assert!(m.matches(
            &key_code(KeyCode::Enter, KeyModifiers::NONE),
            "tui.input.submit"
        ));
        assert!(m.matches(&key_code(KeyCode::Esc, KeyModifiers::NONE), "app.interrupt"));
        // ctrl+letter via the CONTROL flag.
        assert!(m.matches(
            &key_mod('w', KeyModifiers::CONTROL),
            "tui.editor.deleteWordBackward"
        ));
        assert!(m.matches(
            &key_mod('d', KeyModifiers::ALT),
            "tui.editor.deleteWordForward"
        ));
        // ctrl+- (symbol base with ctrl).
        assert!(m.matches(&key_mod('-', KeyModifiers::CONTROL), "tui.editor.undo"));
    }

    #[test]
    fn rebind_changes_behavior() {
        let user = config(&[
            ("tui.input.newLine", vec!["ctrl+m".into()]),
            ("tui.editor.deleteWordBackward", vec!["ctrl+y".into()]),
        ]);
        let m = KeybindingManager::new(&user).unwrap();
        // The old default no longer binds the action...
        assert!(!m.matches(
            &key_code(KeyCode::Enter, KeyModifiers::SHIFT),
            "tui.input.newLine"
        ));
        assert!(!m.matches(
            &key_mod('w', KeyModifiers::CONTROL),
            "tui.editor.deleteWordBackward"
        ));
        // ...and the new keys do.
        assert!(m.matches(&key_mod('m', KeyModifiers::CONTROL), "tui.input.newLine"));
        assert!(m.matches(
            &key_mod('y', KeyModifiers::CONTROL),
            "tui.editor.deleteWordBackward"
        ));
        // Unbinding via an empty list.
        let user = config(&[("tui.input.submit", vec![])]);
        let m = KeybindingManager::new(&user).unwrap();
        assert!(!m.matches(
            &key_code(KeyCode::Enter, KeyModifiers::NONE),
            "tui.input.submit"
        ));
    }

    #[test]
    fn invalid_binding_errors_list_valid_actions() {
        let err = KeybindingManager::new(&config(&[("not.an.action", vec!["ctrl+z".into()])]))
            .unwrap_err();
        assert!(err.contains("not.an.action"), "{err}");
        assert!(
            err.contains("tui.input.submit"),
            "lists valid actions: {err}"
        );
        let err =
            KeybindingManager::new(&config(&[("tui.input.submit", vec!["ctrl+ctrl+q".into()])]))
                .unwrap_err();
        assert!(err.contains("ctrl+ctrl+q"), "{err}");
        assert!(err.contains("tui.input.submit"), "{err}");
    }

    #[test]
    fn conflict_detection_is_user_only() {
        // Default collisions are not conflicts (upstream semantics).
        let m = KeybindingManager::default();
        assert!(m.conflicts().is_empty(), "{:?}", m.conflicts());
        // User bindings to the same key are.
        let user = config(&[
            ("app.suspend", vec!["ctrl+z".into()]),
            ("app.session.new", vec!["ctrl+z".into()]),
        ]);
        let m = KeybindingManager::new(&user).unwrap();
        assert_eq!(m.conflicts().len(), 1, "{:?}", m.conflicts());
        let (key, actions) = &m.conflicts()[0];
        assert_eq!(key, "ctrl+z");
        assert_eq!(actions, &["app.session.new", "app.suspend"]);
    }

    #[test]
    fn legacy_names_migrate() {
        let user = config(&[("submit", vec!["ctrl+enter".into()])]);
        let m = KeybindingManager::new(&user).unwrap();
        assert_eq!(m.get_keys("tui.input.submit"), vec!["ctrl+enter"]);
        // Migration must not break an explicit namespaced entry.
        let user = config(&[
            ("newLine", vec!["ctrl+m".into()]),
            ("tui.input.newLine", vec!["ctrl+n".into()]),
        ]);
        let m = KeybindingManager::new(&user).unwrap();
        assert_eq!(m.get_keys("tui.input.newLine"), vec!["ctrl+n"]);
    }

    #[test]
    fn load_from_agent_dir() {
        let dir = std::env::temp_dir().join(format!("pi-rust-kb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Missing file -> defaults.
        let m = KeybindingManager::load(&dir).unwrap();
        assert_eq!(m.get_keys("tui.input.submit"), vec!["enter"]);
        // Valid file -> applied.
        std::fs::write(
            dir.join("keybindings.json"),
            r#"{"tui.input.submit":"ctrl+enter"}"#,
        )
        .unwrap();
        let m = KeybindingManager::load(&dir).unwrap();
        assert_eq!(m.get_keys("tui.input.submit"), vec!["ctrl+enter"]);
        // Invalid file -> error listing valid actions.
        std::fs::write(dir.join("keybindings.json"), r#"{"nope":"ctrl+z"}"#).unwrap();
        let err = KeybindingManager::load(&dir).unwrap_err();
        assert!(
            err.contains("nope") && err.contains("tui.input.submit"),
            "{err}"
        );
        // Malformed JSON -> error.
        std::fs::write(dir.join("keybindings.json"), "not json").unwrap();
        assert!(KeybindingManager::load(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
