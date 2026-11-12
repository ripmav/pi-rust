//! WP-601: TUI skeleton (ratatui). The app state, key handling, and rendering
//! live here and are fully in-process testable (key injection + buffer
//! snapshots, no TTY). The terminal adapter (crossterm) is in `terminal`.
//!
//! Terminal-craft floor: one accent role, dense chrome, single-row status
//! bar, no boxes-in-boxes, `…` truncation in the status bar.
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap, Widget};
use ratatui::buffer::Buffer;
use serde_json::{json, Value};

pub mod dialog;
pub mod commands;
pub mod keybindings;
pub mod picker;
pub mod markdown;
pub mod theme;
pub mod terminal;

use crossterm::event::{Event as TermEvent, KeyCode, KeyModifiers};

/// WP-609: which picker is open (routes the selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerKind {
	#[default]
	Model,
	Session,
}

/// One transcript line. Roles: `user`, `assistant`, `tool`, `system`.
#[derive(Debug, Clone)]
pub struct Msg {
    pub role: &'static str,
    pub text: String,
}

pub struct TuiApp {
    pub transcript: Vec<Msg>,
    /// Multi-line input (WP-605): '\n'-separated, cursor at (line, col).
    pub input: String,
    pub cursor: (usize, usize),
    /// Submitted messages, newest last (WP-605 history).
    pub history: Vec<String>,
    /// Position inside the history while recalling (None = free editing).
    pub history_idx: Option<usize>,
    /// Draft saved before the first history recall of this editing cycle.
    pub draft: Option<String>,
    /// Model is working (a turn is in flight).
    pub pending: bool,
    pub model: String,
    pub cwd: String,
    pub session: String,
    /// Window title set by an extension (`setTitle`); shown in the status bar.
    pub title: Option<String>,
    /// Status text set by an extension (`setStatus`); shown in the status bar.
    pub status_text: Option<String>,
    /// Open dialog (WP-604); keys are routed to it until it closes.
    pub dialog: Option<dialog::Dialog>,
    /// Open `editor` dialog (WP-605): the main editor pane is reused as the
    /// dialog input; Enter answers, Esc cancels.
    pub editor_session: Option<EditorSession>,
    /// WP-606: upstream keybinding schema + user configuration.
    pub kb: keybindings::KeybindingManager,
    /// WP-607: the active theme (built-in or user theme).
    pub theme: theme::Theme,
    /// WP-608: extension `registerCommand` commands (from the ready payload).
    pub ext_commands: Vec<commands::Command>,
    /// WP-705: (key, description) extension shortcuts from ready — the
    /// sidecar already applied the upstream precedence resolution.
    pub ext_shortcuts: Vec<(String, String)>,
    /// WP-608: prompt templates (WP-307 discovery).
    pub templates: Vec<crate::templates::Template>,
    /// WP-608: the open slash menu (matched commands + selection).
    pub slash: Option<(Vec<commands::Command>, usize)>,
    /// WP-609: the open picker (session resume / model switch).
    pub picker: Option<picker::Picker>,
    /// WP-609: what the open picker selects (its outcome routing).
    pub picker_kind: PickerKind,
    /// Lines scrolled up from the bottom (0 = pinned to newest).
    pub scroll: usize,
    pub size: (u16, u16),
    pub exit: bool,
    pub last_error: Option<String>,
}

/// An `editor` dialog rendered through the editor pane (WP-605).
#[derive(Debug, Clone)]
pub struct EditorSession {
    pub id: String,
    pub title: String,
    pub deadline: std::time::Instant,
}

/// What a terminal key produced (WP-605).
#[derive(Debug, Clone, PartialEq)]
pub enum TuiAction {
    /// Normal submit: the message text (multi-line kept as '\n').
    Submit(String),
    /// An `editor` dialog closed; the caller sends the outcome over the
    /// bridge (WP-604 outcome type).
    EditorClosed(dialog::DialogOutcome),
    /// WP-607: `/theme <name>` — switch the active theme in place.
    SwitchTheme(String),
    /// WP-608: an extension command over the bridge (M1 `command` path).
    BridgeCommand { name: String, args: String },
    /// WP-608: `/thinking <level>` over the bridge (WP-406 request).
    SetThinking { level: String },
    /// WP-608: `/model <provider/model>` over the bridge (WP-406 request).
    SetModel { provider: String, model: String },
    /// WP-609: `/resume` — the terminal loads the session list and opens
    /// the picker (it owns the agent dir).
    OpenSessionsPicker,
    /// WP-705: a registered extension shortcut (ready.shortcuts); the key
    /// string as registered (the terminal sends it back over the bridge).
    BridgeShortcut { key: String },
}

impl TuiApp {
    pub fn new(model: impl Into<String>, cwd: impl Into<String>, session: impl Into<String>) -> Self {
        Self::with_kb(keybindings::KeybindingManager::default(), model, cwd, session)
    }

    /// WP-607: set the active theme (returns self for construction chains).
    pub fn with_theme(mut self, theme: theme::Theme) -> Self {
        self.theme = theme;
        self
    }

    /// WP-608: set the extension commands and prompt templates that feed
    /// the slash menu and dispatch.
    /// WP-705: the resolved extension shortcuts from `ready`.
    pub fn with_shortcuts(mut self, ext_shortcuts: Vec<(String, String)>) -> Self {
        self.ext_shortcuts = ext_shortcuts;
        self
    }

    pub fn with_commands(mut self, ext_commands: Vec<commands::Command>, templates: Vec<crate::templates::Template>) -> Self {
        self.ext_commands = ext_commands;
        self.templates = templates;
        self
    }

    /// WP-608: recompute the slash menu for the current input. Called on
    /// key events (never in the render path, per the house rules).
    fn refresh_slash(&mut self) {
        if self.editor_session.is_some() {
            self.slash = None;
            return;
        }
        let all = commands::collect_menu(&self.templates, &self.ext_commands);
        self.slash = commands::menu_items(&all, &self.input).map(|items| (items, 0));
    }

    /// WP-609: open the model picker (bundled registry, `provider/model`
    /// labels, sorted).
    pub fn open_model_picker(&mut self) {
        let reg = crate::registry::ModelRegistry::bundled();
        let mut items: Vec<picker::PickerItem> = Vec::new();
        for p in reg.providers() {
            for m in reg.models_of(p) {
                let label = format!("{}/{}", m.provider, m.id);
                items.push(picker::PickerItem { display: label.clone(), value: label });
            }
        }
        items.sort_by(|a, b| a.display.cmp(&b.display));
        self.picker_kind = PickerKind::Model;
        self.slash = None;
        self.picker = Some(picker::Picker::new("Select model", items));
    }

    /// WP-609: route one key to the open picker (the terminal calls this
    /// before the normal key handling; a closing outcome is returned).
    pub fn handle_picker_key(&mut self, ev: &TermEvent) -> Option<picker::PickerOutcome> {
        let k = match ev {
            TermEvent::Key(k) => k,
            _ => return None,
        };
        let kb = &self.kb;
        self.picker.as_mut()?.handle_key(k, kb)
    }

    /// WP-608: push one command result line to the transcript (dim system
    /// role).
    fn push_command_output(&mut self, text: String) {
        self.transcript.push(Msg { role: "system", text });
        self.scroll = 0;
    }

    /// WP-608: a command error in the status bar + core log.
    fn set_cmd_error(&mut self, msg: &str) {
        crate::corelog::line(&format!("[tui] {msg}"));
        self.last_error = Some(msg.to_string());
    }

    /// WP-608: `/hotkeys` output — the wired keybindings as a markdown
    /// table (upstream renders the same from the keybinding schema).
    fn hotkeys_text(&self) -> String {
        let mut out = String::from("Keybindings (v0 wired actions):\n");
        for action in [
            "tui.input.submit", "tui.input.newLine", "app.clear", "app.exit",
            "tui.select.up", "tui.select.down", "tui.select.confirm", "tui.select.cancel",
            "tui.editor.cursorUp", "tui.editor.cursorDown", "tui.editor.cursorLeft",
            "tui.editor.cursorRight",
        ] {
            let keys = self.kb.get_keys(action).iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(" / ");
            out.push_str(&format!("- {action}: {keys}\n"));
        }
        out
    }

    /// WP-608: dispatch a submitted `/name args`. Returns the produced
    /// action; `None` means the command was handled locally (session
    /// info, hotkeys, errors, /quit) and the input was cleared.
    fn dispatch_command(&mut self, name: &str, args: &str) -> Option<TuiAction> {
        let clear = |this: &mut Self| {
            this.input.clear();
            this.cursor = (0, 0);
            this.leave_history();
            this.scroll = 0;
        };
        match name {
            "theme" => {
                self.input.clear();
                self.cursor = (0, 0);
                self.leave_history();
                self.scroll = 0;
                return Some(TuiAction::SwitchTheme(args.to_string()));
            }
            "quit" => {
                self.exit = true;
                self.input.clear();
                return None;
            }
            "session" => {
                self.push_command_output(format!(
                    "session: {} | model: {} | cwd: {}",
                    self.session, self.model, self.cwd
                ));
                clear(self);
                return None;
            }
            "hotkeys" => {
                self.push_command_output(self.hotkeys_text());
                clear(self);
                return None;
            }
            "thinking" => {
                if args.is_empty() {
                    self.set_cmd_error("usage: /thinking <level>");
                    return None;
                }
                self.history.push(format!("/thinking {args}"));
                self.history_idx = None;
                self.draft = None;
                self.input.clear();
                self.cursor = (0, 0);
                self.scroll = 0;
                return Some(TuiAction::SetThinking { level: args.to_string() });
            }
            "model" => {
                if args.is_empty() {
                    // WP-609: no argument opens the model picker (upstream
                    // opens the selector UI here too).
                    self.open_model_picker();
                    return None;
                }
                if let Some((provider, model)) = args.split_once('/') {
                    self.history.push(format!("/model {args}"));
                    self.history_idx = None;
                    self.draft = None;
                    self.input.clear();
                    self.cursor = (0, 0);
                    self.scroll = 0;
                    return Some(TuiAction::SetModel { provider: provider.to_string(), model: model.to_string() });
                }
                self.set_cmd_error("usage: /model <provider/model>");
                return None;
            }
            _ => {}
        }
        // Prompt template (WP-307): expand and submit as a normal message.
        if let Some(t) = self.templates.iter().find(|t| t.name == name) {
            let expanded = crate::templates::expand(&t.content, &crate::templates::parse_args(args));
            self.history.push(format!("/{name} {args}"));
            self.history_idx = None;
            self.draft = None;
            self.input.clear();
            self.cursor = (0, 0);
            self.scroll = 0;
            return Some(TuiAction::Submit(expanded));
        }
        // Extension command: the M1 bridge `command` path.
        if self.ext_commands.iter().any(|c| c.name == name) {
            self.history.push(format!("/{name} {args}"));
            self.history_idx = None;
            self.draft = None;
            self.input.clear();
            self.cursor = (0, 0);
            self.scroll = 0;
            return Some(TuiAction::BridgeCommand { name: name.to_string(), args: args.to_string() });
        }
        // WP-609: /resume opens the session picker (the terminal loads the
        // list — it owns the agent dir).
        if name == "resume" {
            self.input.clear();
            self.cursor = (0, 0);
            self.leave_history();
            self.scroll = 0;
            return Some(TuiAction::OpenSessionsPicker);
        }
        // Known built-in, not wired in v0 (upstream has a full
        // implementation; v0 says so explicitly).
        if commands::BUILTIN_SLASH_COMMANDS.iter().any(|(n, _, _)| *n == name) {
            self.set_cmd_error(&format!("/{name} is not supported in v0"));
            return None;
        }
        // Unknown command: error listing the available ones (spec).
        let all = commands::collect_menu(&self.templates, &self.ext_commands);
        let names: Vec<String> = all.iter().map(|c| format!("/{}", c.name)).collect();
        self.set_cmd_error(&format!("unknown command /{name}; available: {}", names.join(" ")));
        None
    }

    /// WP-606: construct with a configured keybinding manager.
    pub fn with_kb(
        kb: keybindings::KeybindingManager,
        model: impl Into<String>,
        cwd: impl Into<String>,
        session: impl Into<String>,
    ) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            cursor: (0, 0),
            history: Vec::new(),
            history_idx: None,
            draft: None,
            pending: false,
            model: model.into(),
            cwd: cwd.into(),
            session: session.into(),
            title: None,
            status_text: None,
            dialog: None,
            editor_session: None,
            kb,
            theme: theme::Theme::builtin("dark").expect("built-in dark theme"),
            ext_commands: Vec::new(),
            ext_shortcuts: Vec::new(),
            templates: Vec::new(),
            slash: None,
            picker: None,
            picker_kind: PickerKind::default(),
            scroll: 0,
            size: (80, 24),
            exit: false,
            last_error: None,
        }
    }

    pub fn set_size(&mut self, w: u16, h: u16) {
        if w > 0 && h > 0 {
            self.size = (w, h);
        }
    }

    /// Open a dialog from a bridge `extension_ui_request` (dialog subset).
    pub fn open_dialog(&mut self, req: &Value) -> bool {
        match dialog::Dialog::from_request(req) {
            Some(d) => {
                self.dialog = Some(d);
                true
            }
            None => false,
        }
    }

    /// Route one key to the open dialog. Returns the outcome when the dialog
    /// closes; `None` while it stays open (key consumed) or no dialog is open.
    pub fn handle_dialog_key(&mut self, ev: &TermEvent) -> Option<dialog::DialogOutcome> {
        let d = self.dialog.as_mut()?;
        let outcome = d.handle_key(ev)?;
        self.dialog = None;
        Some(outcome)
    }

    /// Open an `editor` dialog in the editor pane (WP-605): the pane shows
    /// the prefilled text; Enter answers with the pane text, Esc cancels.
    pub fn open_editor_session(&mut self, req: &Value) -> bool {
        let (Some(id), Some(title), Some(method)) = (
            req.get("id").and_then(Value::as_str),
            req.get("title").and_then(Value::as_str),
            req.get("method").and_then(Value::as_str),
        ) else {
            return false;
        };
        if method != "editor" {
            return false;
        }
        let prefill = req.get("prefill").and_then(Value::as_str).unwrap_or_default();
        let timeout_ms = req.get("timeout").and_then(Value::as_u64).unwrap_or(30_000);
        self.input = prefill.to_string();
        let (l, c) = self
            .input
            .rsplit('\n')
            .next()
            .map(|last| (self.input.matches('\n').count(), last.chars().count()))
            .unwrap_or((0, 0));
        self.cursor = (l, c);
        self.history_idx = None;
        self.editor_session = Some(EditorSession {
            id: id.to_string(),
            title: title.to_string(),
            deadline: std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms),
        });
        true
    }

    /// Close a dialog (or editor session) whose sidecar-side timeout has
    /// expired (no response — the sidecar already resolved with its
    /// upstream default).
    pub fn take_expired_dialog(&mut self, now: std::time::Instant) -> Option<dialog::DialogOutcome> {
        if self.dialog.as_ref().map(|d| d.expired(now)).unwrap_or(false) {
            let d = self.dialog.take().expect("expired check just matched");
            return Some(d.timed_out());
        }
        if self
            .editor_session
            .as_ref()
            .map(|e| e.deadline <= now)
            .unwrap_or(false)
        {
            let es = self.editor_session.take().expect("expired check just matched");
            return Some(dialog::DialogOutcome::TimedOut {
                id: es.id,
                method: "editor".into(),
                title: es.title,
                message: String::new(),
            });
        }
        None
    }

    // ---- editor pane operations (WP-605) ----

    /// Run one edit: the input is split into lines, f mutates lines +
    /// cursor, then both are written back.
    fn apply_edit<F: FnOnce(&mut Vec<String>, &mut (usize, usize))>(&mut self, f: F) {
        let mut lines: Vec<String> = self.input.split('\n').map(String::from).collect();
        let mut cur = self.cursor;
        f(&mut lines, &mut cur);
        self.input = lines.join("\n");
        self.cursor = cur;
    }

    /// Typing/editing after a history recall starts a fresh draft (the
    /// recalled entry is not modified).
    fn leave_history(&mut self) {
        if self.history_idx.is_some() {
            self.history_idx = None;
            self.draft = None;
        }
    }

    fn insert_char_at(&mut self, c: char) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            let line = lines.get_mut(*l).cloned().unwrap_or_default();
            let mut chars: Vec<char> = line.chars().collect();
            chars.insert(*col, c);
            if let Some(slot) = lines.get_mut(*l) {
                *slot = chars.into_iter().collect();
            }
            *col += 1;
        });
    }

    fn insert_newline(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            let line = lines.get(*l).cloned().unwrap_or_default();
            let mut chars: Vec<char> = line.chars().collect();
            let (head, tail) = chars.split_at(*col);
            let head: Vec<char> = head.to_vec();
            let tail: Vec<char> = tail.to_vec();
            if let Some(slot) = lines.get_mut(*l) {
                *slot = head.into_iter().collect();
            }
            lines.insert(*l + 1, tail.into_iter().collect());
            *l += 1;
            *col = 0;
        });
    }

    fn backspace_at(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            if *col > 0 {
                if let Some(line) = lines.get_mut(*l) {
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.pop();
                    *line = chars.into_iter().collect();
                }
                *col -= 1;
            } else if *l > 0 {
                // Merge with the previous line (delete the '\n').
                let prev_len = lines[*l - 1].chars().count();
                let cur_line = lines.get(*l).cloned().unwrap_or_default();
                if let Some(prev) = lines.get_mut(*l - 1) {
                    prev.push_str(&cur_line);
                }
                lines.remove(*l);
                *l -= 1;
                *col = prev_len;
            }
        });
    }

    fn delete_char_forward(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            if let Some(line) = lines.get(*l) {
                if *col < line.chars().count() {
                    let mut chars: Vec<char> = line.chars().collect();
                    chars.remove(*col);
                    lines[*l] = chars.into_iter().collect();
                    return;
                }
            }
            if *l + 1 < lines.len() {
                // Merge the next line into this one (delete the '\n').
                let next = lines.remove(*l + 1);
                lines[*l].push_str(&next);
            }
        });
    }

    fn delete_word_backward(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            let mut i = *l;
            let mut c = *col;
            // Walk backward over whitespace (may cross lines).
            loop {
                let line = lines.get(i).map(|s| s.as_str()).unwrap_or("");
                let chars: Vec<char> = line.chars().collect();
                if c > 0 && chars[c - 1].is_whitespace() {
                    c -= 1;
                    continue;
                }
                if i > 0 && c == 0 {
                    i -= 1;
                    c = lines[i].chars().count();
                    continue;
                }
                break;
            }
            // Then over word chars.
            loop {
                let line = lines.get(i).map(|s| s.as_str()).unwrap_or("");
                let chars: Vec<char> = line.chars().collect();
                if c > 0 && !chars[c - 1].is_whitespace() {
                    c -= 1;
                    continue;
                }
                break;
            }
            if (i, c) == (*l, *col) {
                return;
            }
            // Delete from (i,c) to (l,col): keep the prefix of the first
            // affected line and the suffix of the last.
            let first: String = lines[i].chars().take(c).collect();
            let last: String = lines[*l].chars().skip(*col).collect();
            let mut new: Vec<String> = lines[..i].to_vec();
            new.push(format!("{first}{last}"));
            *lines = new;
            *l = i;
            *col = c;
        });
    }

    fn delete_word_forward(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (l, col)| {
            let mut i = *l;
            let mut c = *col;
            loop {
                let line = lines.get(i).map(|s| s.as_str()).unwrap_or("");
                let chars: Vec<char> = line.chars().collect();
                if c < chars.len() && chars[c].is_whitespace() {
                    c += 1;
                    continue;
                }
                if i + 1 < lines.len() && c == chars.len() {
                    i += 1;
                    c = 0;
                    continue;
                }
                break;
            }
            loop {
                let line = lines.get(i).map(|s| s.as_str()).unwrap_or("");
                let chars: Vec<char> = line.chars().collect();
                if c < chars.len() && !chars[c].is_whitespace() {
                    c += 1;
                    continue;
                }
                break;
            }
            if (i, c) == (*l, *col) {
                return;
            }
            let first: String = lines[*l].chars().take(*col).collect();
            let rest: String = lines[i].chars().skip(c).collect();
            let mut new: Vec<String> = lines[..*l].to_vec();
            new.push(format!("{first}{rest}"));
            *lines = new;
            *l = *l; // stays on the (now merged) first line
            *col = *col;
        });
    }

    fn delete_to_line_start(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (_l, col)| {
            if let Some(line) = lines.get_mut(*_l) {
                let rest: String = line.chars().skip(*col).collect();
                *line = rest;
            }
        });
    }

    fn delete_to_line_end(&mut self) {
        self.leave_history();
        self.apply_edit(|lines, (_l, col)| {
            if let Some(line) = lines.get_mut(*_l) {
                let mut chars: Vec<char> = line.chars().collect();
                chars.truncate(*col);
                *line = chars.into_iter().collect();
            }
        });
    }

    fn move_cursor_left(&mut self) {
        if self.cursor.1 > 0 {
            self.cursor.1 -= 1;
        } else if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.input.split('\n').nth(self.cursor.0).map(|s| s.chars().count()).unwrap_or(0);
        }
    }

    fn move_cursor_right(&mut self) {
        let len = self.input.split('\n').nth(self.cursor.0).map(|s| s.chars().count()).unwrap_or(0);
        if self.cursor.1 < len {
            self.cursor.1 += 1;
        } else if self.cursor.0 + 1 < self.input.lines().count().max(1) {
            self.cursor.0 += 1;
            self.cursor.1 = 0;
        }
    }

    fn move_cursor_home(&mut self) {
        self.cursor.1 = 0;
    }

    fn move_cursor_end(&mut self) {
        self.cursor.1 = self.input.split('\n').nth(self.cursor.0).map(|s| s.chars().count()).unwrap_or(0);
    }

    fn close_editor_session(&mut self, response: Value) -> TuiAction {
        let Some(es) = self.editor_session.take() else {
            unreachable!("close_editor_session without an open session");
        };
        self.input.clear();
        self.cursor = (0, 0);
        TuiAction::EditorClosed(dialog::DialogOutcome::Answered {
            id: es.id,
            method: "editor".into(),
            title: es.title,
            message: String::new(),
            response,
        })
    }

    fn cancel_response(id: &str) -> Value {
        json!({"type":"extension_ui_response","id":id,"cancelled":true})
    }

    /// Bracketed paste at the cursor (WP-605): multi-line text keeps its
    /// line structure; mid-line pastes split the line.
    fn paste_at(&mut self, text: &str) {
        self.leave_history();
        self.apply_edit(|lines, cursor| {
            let (mut l, mut col) = (cursor.0, cursor.1);
            let line = lines.get(l).cloned().unwrap_or_default();
            let mut chars: Vec<char> = line.chars().collect();
            let head: String = chars.drain(..col).collect();
            let tail: String = chars.into_iter().collect();
            let mut parts: Vec<String> = text.split('\n').map(String::from).collect();
            let first = parts.remove(0);
            if parts.is_empty() {
                if let Some(slot) = lines.get_mut(l) {
                    *slot = format!("{head}{first}{tail}");
                }
                col = head.chars().count() + first.chars().count();
            } else {
                // The first part replaces the cursor position; the remaining
                // parts become new lines; the restored tail joins the last.
                if let Some(slot) = lines.get_mut(l) {
                    *slot = format!("{head}{first}");
                }
                let tail_len = tail.chars().count();
                let mut new_lines = parts;
                if text.ends_with('\n') {
                    // Paste ends in a newline: the restored tail is its own
                    // line after the pasted block.
                    new_lines.push(tail);
                } else {
                    // The tail continues the last pasted line.
                    new_lines.last_mut().expect("paste always has parts").push_str(&tail);
                }
                for (j, p) in new_lines.iter().enumerate() {
                    lines.insert(l + 1 + j, p.clone());
                }
                let last_idx = l + new_lines.len();
                l = last_idx;
                if text.ends_with('\n') {
                    l = last_idx + 1;
                    col = 0;
                } else {
                    col = lines[last_idx].chars().count() - tail_len;
                }
            }
            cursor.0 = l;
            cursor.1 = col;
        });
    }

    /// History recall (WP-605): up walks into the history, down back out to
    /// the saved draft.
    fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        match self.history_idx {
            None => {
                self.draft = Some(self.input.clone());
                self.history_idx = Some(self.history.len() - 1);
            }
            Some(i) if i > 0 => {
                self.history_idx = Some(i - 1);
            }
            _ => return false,
        }
        let i = self.history_idx.unwrap();
        self.input = self.history[i].clone();
        self.cursor = (0, self.input.chars().count());
        true
    }

    fn history_next(&mut self) -> bool {
        let Some(i) = self.history_idx else {
            return false;
        };
        if i + 1 < self.history.len() {
            self.history_idx = Some(i + 1);
            self.input = self.history[i + 1].clone();
            self.cursor = (0, self.input.chars().count());
        } else {
            self.history_idx = None;
            self.input = self.draft.take().unwrap_or_default();
            self.cursor = (0, self.input.chars().count());
        }
        true
    }

    fn submit_text(&mut self) -> Option<String> {
        let t = self.input.trim().to_string();
        if t.is_empty() {
            return None;
        }
        self.history.push(t.clone());
        self.history_idx = None;
        self.draft = None;
        self.input.clear();
        self.cursor = (0, 0);
        Some(t)
    }

    /// Handle one terminal event. Returns the produced action (submit or an
    /// editor-dialog close). While a dialog is open it owns the keys (the
    /// terminal loop routes them via `handle_dialog_key` first; the guard
    /// covers direct calls).
    ///
    /// WP-605 editor keys (upstream default bindings): Enter submit,
    /// shift+Enter / ctrl+j newline, ctrl+w / alt+Backspace word backward,
    /// alt+d word forward, ctrl+u / ctrl+k to line start / end,
    /// Delete / ctrl+d char forward (ctrl+d still exits with an empty
    /// editor outside editor sessions), left/right/home/end caret.
    pub fn handle_key(&mut self, ev: TermEvent) -> Option<TuiAction> {
        if self.dialog.is_some() {
            return None;
        }
        match ev {
            TermEvent::Resize(w, h) => {
                self.set_size(w, h);
                None
            }
            TermEvent::Paste(text) => {
                self.paste_at(&text);
                self.scroll = 0;
                None
            }
            TermEvent::Key(k) => {
                // WP-608: recompute the slash menu after the input changes
                // (a key event, never the render path).
                let before = self.input.clone();
                let action = self.process_key(k);
                if self.input != before {
                    self.refresh_slash();
                }
                action
            }
            _ => None,
        }
    }

    fn process_key(&mut self, k: crossterm::event::KeyEvent) -> Option<TuiAction> {
        // WP-705: extension shortcuts dispatch before any built-in binding
        // (upstream custom-editor.handleInput checks them first); a match
        // consumes the key.
        for (key, _) in &self.ext_shortcuts {
            if keybindings::matches_event(&k, key) {
                return Some(TuiAction::BridgeShortcut { key: key.clone() });
            }
        }
        {
            // WP-606: the keybinding manager (upstream schema + user
            // config) drives the wired actions; first match wins.
            let kb = &self.kb;
            // WP-608: an open slash menu owns the select keys (upstream:
            // navigation runs before history/editor actions; escape or
            // ctrl+c closes the menu instead of clearing/exiting).
            if self.slash.is_some() {
                if kb.matches(&k, "tui.select.up") {
                    if let Some((items, sel)) = &mut self.slash {
                        *sel = if *sel == 0 { items.len() - 1 } else { *sel - 1 };
                    }
                    return None;
                }
                if kb.matches(&k, "tui.select.down") {
                    if let Some((items, sel)) = &mut self.slash {
                        *sel = (*sel + 1) % items.len();
                    }
                    return None;
                }
                if kb.matches(&k, "tui.select.confirm") {
                    if let Some((items, sel)) = &self.slash {
                        let name = items.get(*sel).map(|c| c.name.clone());
                        if let Some(name) = name {
                            self.input = format!("/{name} ");
                            self.cursor = (0, name.len() + 2);
                            self.history_idx = None;
                            self.draft = None;
                            self.slash = None;
                        }
                    }
                    return None;
                }
                if kb.matches(&k, "tui.select.cancel") {
                    self.slash = None;
                    return None;
                }
            }
                if kb.matches(&k, "tui.editor.deleteWordBackward") {
                    self.delete_word_backward();
                    return None;
                }
                if kb.matches(&k, "tui.editor.deleteWordForward") {
                    self.delete_word_forward();
                    return None;
                }
                if kb.matches(&k, "tui.editor.deleteToLineStart") {
                    self.delete_to_line_start();
                    return None;
                }
                if kb.matches(&k, "tui.editor.deleteToLineEnd") {
                    self.delete_to_line_end();
                    return None;
                }
                if kb.matches(&k, "tui.editor.cursorLeft") {
                    self.move_cursor_left();
                    return None;
                }
                if kb.matches(&k, "tui.editor.cursorRight") {
                    self.move_cursor_right();
                    return None;
                }
                if kb.matches(&k, "tui.editor.cursorLineStart") {
                    self.move_cursor_home();
                    return None;
                }
                if kb.matches(&k, "tui.editor.cursorLineEnd") {
                    self.move_cursor_end();
                    return None;
                }
                // User-bound history keys (unbound by default upstream; the
                // bare up/down fallback below keeps the v0 behavior).
                if self.editor_session.is_none() {
                    if kb.matches(&k, "tui.editor.historyPrevious") {
                        self.history_prev();
                        return None;
                    }
                    if kb.matches(&k, "tui.editor.historyNext") {
                        self.history_next();
                        return None;
                    }
                }
                // ctrl+c: app.clear (dialogs consume their keys earlier).
                if kb.matches(&k, "app.clear") {
                    if let Some(id) = self.editor_session.as_ref().map(|e| e.id.clone()) {
                        return Some(self.close_editor_session(Self::cancel_response(&id)));
                    }
                    if self.input.trim().is_empty() {
                        self.exit = true;
                    } else {
                        self.input.clear();
                        self.cursor = (0, 0);
                        self.leave_history();
                    }
                    return None;
                }
                // ctrl+d: app.exit when the editor is empty, else
                // deleteCharForward (inside editor sessions always).
                if kb.matches(&k, "app.exit") && self.input.trim().is_empty() {
                    self.exit = true;
                    return None;
                }
                if kb.matches(&k, "tui.editor.deleteCharForward") {
                    self.delete_char_forward();
                    return None;
                }
                // Newline keys (default shift+enter / ctrl+j, rebindable):
                // they win over submit for the same physical Enter.
                if kb.matches(&k, "tui.input.newLine") {
                    self.insert_newline();
                    return None;
                }
                // Enter: in an editor session it answers the dialog, else
                // it submits.
                if k.code == KeyCode::Enter {
                    if self.editor_session.is_some() {
                        let value = self.input.clone();
                        let id = self.editor_session.as_ref().unwrap().id.clone();
                        let response = json!({"type":"extension_ui_response","id":id,"value":value});
                        return Some(self.close_editor_session(response));
                    }
                    if kb.matches(&k, "tui.input.submit") {
                        // WP-608: dispatch /commands before a normal submit
                        // (local builtins, templates, extension commands).
                        let text = self.input.trim().to_string();
                        if let Some((name, args)) = commands::parse_invocation(&text) {
                            return self.dispatch_command(name, args);
                        }
                        if let Some(t) = self.submit_text() {
                            self.scroll = 0;
                            return Some(TuiAction::Submit(t));
                        }
                    }
                    return None;
                }
                // Esc: tui.select.cancel includes escape — closes an editor
                // session; no running turn exists to interrupt in v0.
                if k.code == KeyCode::Esc {
                    if let Some(id) = self.editor_session.as_ref().map(|e| e.id.clone()) {
                        if kb.matches(&k, "tui.select.cancel") {
                            return Some(self.close_editor_session(Self::cancel_response(&id)));
                        }
                    }
                    return None;
                }
                if kb.matches(&k, "tui.editor.deleteCharBackward") {
                    self.backspace_at();
                    return None;
                }
                // v0 fallback (upstream leaves history unbound by default):
                // bare up/down walk the history, or scroll the transcript.
                match k.code {
                    KeyCode::Up
                        if k.modifiers == KeyModifiers::NONE && self.editor_session.is_none() =>
                    {
                        if !self.history_prev() {
                            self.scroll += 1;
                        }
                        None
                    }
                    KeyCode::Down
                        if k.modifiers == KeyModifiers::NONE && self.editor_session.is_none() =>
                    {
                        if !self.history_next() {
                            self.scroll = self.scroll.saturating_sub(1);
                        }
                        None
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        self.insert_char_at(c);
                        self.scroll = 0;
                        None
                    }
                    _ => None,
                }
            }
        }

    /// Apply one core event (the broadcast EventLog stream) to the app state.
    pub fn apply_event(&mut self, ev: &Value) {
        match ev.get("kind").and_then(Value::as_str) {
            Some("user_input") => {
                let text = ev.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                self.transcript.push(Msg { role: "user", text });
                self.scroll = 0;
            }
            Some("model_output") => {
                let text = ev.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
                // WP-603: streamed deltas already built this message — the
                // final event must not duplicate it (non-streaming drivers
                // emit no deltas and push the full text here).
                if self
                    .transcript
                    .last()
                    .is_some_and(|m| m.role == "assistant" && m.text == text)
                {
                    self.scroll = 0;
                    return;
                }
                self.transcript.push(Msg { role: "assistant", text });
                self.scroll = 0;
            }
            Some("model_delta") => {
                // WP-603: append the chunk to the in-flight assistant
                // message (new message if the turn has not started one).
                let text = ev.get("text").and_then(Value::as_str).unwrap_or_default();
                match self.transcript.last_mut() {
                    Some(m) if m.role == "assistant" => m.text.push_str(text),
                    _ => self.transcript.push(Msg {
                        role: "assistant",
                        text: text.to_string(),
                    }),
                }
                self.scroll = 0;
            }
            Some("bridge_cmd_result") => {
                // WP-608: a slash command's bridge round-trip completed.
                let label = ev.get("label").and_then(Value::as_str).unwrap_or("/?");
                let ok = ev.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let text = ev.get("text").and_then(Value::as_str).unwrap_or("");
                let line = if !ok {
                    format!("{label}: {text}")
                } else if text.is_empty() {
                    format!("{label} ok")
                } else {
                    format!("{label} — {text}")
                };
                self.transcript.push(Msg { role: "system", text: line });
                self.scroll = 0;
                if ok {
                    if let Some(model) = ev.get("model").and_then(Value::as_str) {
                        self.model = model.to_string();
                    }
                }
            }
            Some("tool_execution") => {
                let name = ev.get("name").and_then(Value::as_str).unwrap_or("?");
                let blocked = ev.get("blocked").and_then(Value::as_bool).unwrap_or(false);
                let summary = ev
                    .get("result_summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .split('\n')
                    .next()
                    .unwrap_or("");
                let text = if blocked {
                    format!("{name}: blocked — {summary}")
                } else {
                    format!("{name}: {summary}")
                };
                self.transcript.push(Msg { role: "tool", text });
                self.scroll = 0;
            }
            Some("agent_start") => self.pending = true,
            Some("agent_end") => {
                self.pending = false;
                self.last_error = ev
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("extension_error") => {
                self.last_error = ev
                    .get("message")
                    .or_else(|| ev.get("error"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            // WP-604: fire-and-forget UI channel (dialogs arrive over the ui
            // channel, not the broadcast).
            Some("notify") => {
                let message = ev.get("message").and_then(Value::as_str).unwrap_or_default();
                self.transcript.push(Msg {
                    role: "system",
                    text: format!("ℹ {message}"),
                });
                self.scroll = 0;
            }
            Some("set_status") => {
                self.status_text = ev.get("statusText").and_then(Value::as_str).map(str::to_string);
            }
            Some("set_title") => {
                self.title = ev.get("title").and_then(Value::as_str).map(str::to_string);
            }
            Some("set_editor_text") => {
                if let Some(t) = ev.get("text").and_then(Value::as_str) {
                    self.input = t.to_string();
                    let (l, c) = self
                        .input
                        .rsplit('\n')
                        .next()
                        .map(|last| (self.input.matches('\n').count(), last.chars().count()))
                        .unwrap_or((0, 0));
                    self.cursor = (l, c);
                }
            }
            _ => {} // decision: handled by the core, not the display.
        }
    }

    /// Render into the given buffer at the given area (in-process testable).
    pub fn render_to(&self, buf: &mut Buffer, area: Rect) {
        let (w, h) = (area.width as usize, area.height as usize);
        let status_h = 1usize.min(h);
        // WP-605: the editor pane grows with the input (title + hint rows
        // while an `editor` dialog runs in the pane).
        let input_rows = self.input.matches('\n').count() + 1;
        let editor_chrome = if self.editor_session.is_some() { 2 } else { 0 };
        // WP-608: the slash menu (up to 8 rows) grows the input pane.
        let menu_rows = self.slash.as_ref().map(|(items, _)| items.len().min(8)).unwrap_or(0);
        let input_h = (input_rows + editor_chrome + menu_rows).min(h.saturating_sub(status_h).max(1));
        let transcript_h = h.saturating_sub(status_h).saturating_sub(input_h);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(transcript_h as u16),
                Constraint::Length(input_h as u16),
                Constraint::Length(status_h as u16),
            ])
            .split(area);

        // Transcript: role-marked, word-wrapped, scrolled from the bottom.
        // user/assistant messages render as markdown (WP-602); tool/system
        // lines stay plain. While a dialog is open it occupies this area
        // (WP-604).
        let mut lines: Vec<Line> = Vec::new();
        if let Some(p) = &self.picker {
            // WP-609: the picker occupies the transcript area.
            lines.extend(p.render_rows(
                self.theme.fg_bold("accent"),
                Style::default(),
                self.theme.fg("muted"),
            ));
        } else if let Some(d) = &self.dialog {
            lines.push(Line::from(Span::raw("")));
            lines.extend(d.render_lines());
        } else {
            for m in &self.transcript {
                let (prefix, style): (&str, Style) = match m.role {
                    // WP-607: role prefixes from the active theme.
                    "user" => ("▸ ", self.theme.fg_bold("userMessageText")),
                    "assistant" => ("", Style::default()),
                    "tool" => ("⚙ ", self.theme.fg("toolOutput")),
                    _ => ("· ", self.theme.fg("muted")),
                };
                let body: Vec<Line> = if matches!(m.role, "user" | "assistant") {
                    markdown::md_lines(&m.text, &self.theme)
                } else {
                    m.text.split('\n').map(markdown::plain_line).collect()
                };
                for (i, bl) in body.into_iter().enumerate() {
                    if i == 0 && !prefix.is_empty() {
                        let mut spans = vec![Span::styled(prefix, style.clone())];
                        spans.extend(bl.spans);
                        lines.push(Line::from(spans));
                    } else {
                        lines.push(bl);
                    }
                }
            }
        }
        if self.dialog.is_none()
            && self.pending
            && lines
                .last()
                .and_then(|l| l.spans.last())
                .map(|s| s.content.as_ref().is_empty())
                .unwrap_or(true)
        {
            lines.push(Line::from(Span::styled("● ", self.theme.fg("accent"))));
        }
        let total = lines.len();
        let start = total.saturating_sub(transcript_h).saturating_sub(self.scroll);
        let visible: Vec<Line> = lines[start..total.min(start + transcript_h)].to_vec();
        Paragraph::new(visible)
            .wrap(Wrap { trim: true })
            .render(chunks[0], buf);

        // Input: the editor pane (WP-605): multi-line, caret at the cursor,
        // prompt on the first line, title + hint rows for an editor dialog.
        if input_h > 0 {
            let accent_bold = self.theme.fg_bold("accent");
            let mut pane: Vec<Line> = Vec::new();
            if let Some(es) = &self.editor_session {
                pane.push(Line::from(Span::styled(es.title.clone(), accent_bold.clone())));
            }
            // WP-608: the slash menu (selected row in the accent role).
            if let Some((items, sel)) = &self.slash {
                for (i, c) in items.iter().take(8).enumerate() {
                    let row = format!(
                        " /{} {}  {}",
                        c.name,
                        c.argument_hint.as_deref().unwrap_or(""),
                        c.description
                    );
                    let style = if i == *sel {
                        self.theme.fg_bold("accent")
                    } else {
                        self.theme.fg("muted")
                    };
                    pane.push(Line::from(Span::styled(row, style)));
                }
            }
            let hint_rows = if self.editor_session.is_some() { 2 } else { 0 };
            let visible = input_h.saturating_sub(hint_rows).saturating_sub(menu_rows);
            let text_lines: Vec<String> = self.input.split('\n').map(String::from).collect();
            for (i, line) in text_lines.iter().take(visible).enumerate() {
                let prompt = if i == 0 { "> " } else { "  " };
                let mut spans = vec![Span::styled(prompt.to_string(), accent_bold.clone())];
                if i == self.cursor.0 {
                    let chars: Vec<char> = line.chars().collect();
                    let c = self.cursor.1.min(chars.len());
                    spans.push(Span::raw(chars[..c].iter().collect::<String>()));
                    spans.push(Span::styled("\u{258c}".to_string(), accent_bold.clone()));
                    spans.push(Span::raw(chars[c..].iter().collect::<String>()));
                } else {
                    spans.push(Span::raw(line.clone()));
                }
                pane.push(Line::from(spans));
            }
            if self.editor_session.is_some() {
                pane.push(Line::from(Span::styled(
                    "enter ok \u{b7} esc cancel",
                    self.theme.fg("dim"),
                )));
            }
            while pane.len() < input_h {
                pane.insert(0, Line::from(Span::raw("")));
            }
            Paragraph::new(pane).render(chunks[1], buf);
        }

        // Status bar: model · cwd · session (+ pending + last error), `…`
        // truncated, one row.
        if status_h > 0 {
            let mut segs: Vec<String> = vec![self.model.clone(), self.cwd.clone(), self.session.clone()];
            if let Some(t) = &self.title {
                segs.push(t.clone());
            }
            if let Some(st) = &self.status_text {
                segs.push(st.clone());
            }
            if self.pending {
                segs.push("● working".into());
            }
            if let Some(e) = &self.last_error {
                segs.push(format!("✗ {e}"));
            }
            let joined = segs.join("  ");
            let mut s = joined;
            if s.chars().count() > w {
                let keep = w.saturating_sub(1);
                let t: String = s.chars().take(keep).collect();
                s = t + "…";
            }
            Paragraph::new(Line::from(Span::styled(s, self.theme.fg("muted"))))
                .render(chunks[2], buf);
        }
    }

    /// Convenience for tests: render at the app's current size and return the
    /// buffer as one string (rows joined with newlines, trailing blanks trimmed).
    pub fn render_string(&self) -> String {
        let mut buf = Buffer::empty(Rect::new(0, 0, self.size.0, self.size.1));
        self.render_to(&mut buf, Rect::new(0, 0, self.size.0, self.size.1));
        buf.content
            .iter()
            .map(|c| c.symbol().to_string())
            .collect::<Vec<_>>()
            .chunks(self.size.0 as usize)
            .map(|row| {
                let r: String = row.concat();
                r.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn key(c: char) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn key_mod(c: char, m: KeyModifiers) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), m))
    }
    fn key_code(code: KeyCode, m: KeyModifiers) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, m))
    }
    fn paste(s: &str) -> TermEvent {
        TermEvent::Paste(s.to_string())
    }
    /// Feed a whole line of printable text through the key handler.
    fn type_str(app: &mut TuiApp, s: &str) {
        for c in s.chars() {
            app.handle_key(key(c));
        }
    }

    #[test]
    fn wp605_paste_50_line_block_no_corruption() {
        let mut app = TuiApp::new("m", "cwd", "s");
        let block: Vec<String> = (1..=50).map(|i| format!("line {:02} with some text", i)).collect();
        app.handle_key(paste(&block.join("\n")));
        let lines: Vec<&str> = app.input.split('\n').collect();
        assert_eq!(lines.len(), 50, "paste must keep every line");
        assert_eq!(lines.first().copied(), Some("line 01 with some text"));
        assert_eq!(lines.last().copied(), Some("line 50 with some text"));
        // Paste mid-text keeps both sides.
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "ab");
        app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(paste("x\ny"));
        assert_eq!(app.input, "ax\nyb");
    }

    #[test]
    fn wp605_history_recall_up_down() {
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "first");
        assert!(is_submit(app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE)), "first"));
        type_str(&mut app, "second");
        assert!(is_submit(app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE)), "second"));
        assert_eq!(app.input, "");
        app.handle_key(key_code(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "second", "up recalls the newest");
        app.handle_key(key_code(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "first", "up recalls the older");
        app.handle_key(key_code(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "second", "down walks forward");
        app.handle_key(key_code(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "", "down past the end restores the draft");
    }

    #[test]
    fn wp605_multiline_submit_and_shift_enter_newline() {
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "one");
        assert!(app.handle_key(key_code(KeyCode::Enter, KeyModifiers::SHIFT)).is_none());
        assert!(app.input.contains('\n'), "shift+enter inserts a newline");
        type_str(&mut app, "two");
        assert!(is_submit(app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE)), "one\ntwo"));
        assert_eq!(app.input, "");
        // Empty submit stays a no-op.
        assert!(app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn wp605_word_wise_editing() {
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "hello world foo");
        // ctrl+w: delete word backward
        app.handle_key(key_code(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(app.input, "hello world ");
        app.handle_key(key_code(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(app.input, "hello ");
        app.handle_key(key_code(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(app.input, "");
        // alt+backspace too
        type_str(&mut app, "a b");
        app.handle_key(key_code(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(app.input, "a ");
        // alt+d: delete word forward (cursor via left arrow)
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "aa bb cc");
        app.handle_key(key_code(KeyCode::Home, KeyModifiers::NONE));
        app.handle_key(key_code(KeyCode::Char('d'), KeyModifiers::ALT));
        assert_eq!(app.input, " bb cc");
        // ctrl+u / ctrl+k (cursor left of the 'z')
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "xy z");
        for _ in 0..2 { app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE)); }
        app.handle_key(key_code(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(app.input, " z");
        app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(key_code(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(app.input, " ");
    }

    #[test]
    fn wp605_cursor_line_movement() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.handle_key(paste("ab\ncd"));
        assert_eq!((app.cursor.0, app.cursor.1), (1, 2), "caret at end after paste");
        app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE));
        app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!((app.cursor.0, app.cursor.1), (0, 2), "left wraps to previous line end");
        app.handle_key(key_code(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!((app.cursor.0, app.cursor.1), (0, 0));
        app.handle_key(key_code(KeyCode::End, KeyModifiers::NONE));
        assert_eq!((app.cursor.0, app.cursor.1), (0, 2));
        app.handle_key(key('X'));
        assert_eq!(app.input, "abX\ncd", "insert at line-internal cursor");
        app.handle_key(key_code(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.input, "ab\ncd");
        assert_eq!((app.cursor.0, app.cursor.1), (0, 2));
        // Backspace at a line start merges with the previous line.
        for _ in 0..2 { app.handle_key(key_code(KeyCode::Left, KeyModifiers::NONE)); }
        for _ in 0..3 { app.handle_key(key_code(KeyCode::Right, KeyModifiers::NONE)); }
        assert_eq!((app.cursor.0, app.cursor.1), (1, 0), "caret at line-1 start");
        app.handle_key(key_code(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.input, "abcd", "lines merged");
        assert_eq!((app.cursor.0, app.cursor.1), (0, 2));
    }

    #[test]
    fn wp605_editor_dialog_reuses_pane() {
        let mut app = TuiApp::new("m", "cwd", "s");
        let req = json!({"id":"e1","method":"editor","title":"Note","prefill":"l1\nl2","timeout":60000});
        assert!(app.open_editor_session(&req));
        // The editor pane holds the prefilled lines; typing appends.
        assert_eq!(app.input, "l1\nl2");
        type_str(&mut app, "x");
        let action = app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.editor_session.is_none(), "session closes");
        let outcome = editor_outcome(action).expect("editor close");
        match outcome {
            dialog::DialogOutcome::Answered { response, .. } => {
                assert_eq!(response["value"], json!("l1\nl2x"));
                assert_eq!(response["id"], json!("e1"));
            }
            other => panic!("unexpected {other:?}"),
        }
        // Esc cancels.
        let mut app = TuiApp::new("m", "cwd", "s");
        app.open_editor_session(&req);
        let action = app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        let outcome = editor_outcome(action).expect("editor close");
        match outcome {
            dialog::DialogOutcome::Answered { response, .. } => assert_eq!(response["cancelled"], json!(true)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn wp606_rebind_changes_tui_behavior() {
        let user: keybindings::KeybindingsConfig = BTreeMap::from([
            ("tui.input.newLine".to_string(), vec!["ctrl+m".to_string()]),
        ]);
        let kb = keybindings::KeybindingManager::new(&user).unwrap();
        let mut app = TuiApp::with_kb(kb, "m", "cwd", "s");
        type_str(&mut app, "hi");
        // The rebind: ctrl+m now inserts a newline.
        app.handle_key(key_code(KeyCode::Char('m'), KeyModifiers::CONTROL));
        assert!(app.input.contains('\n'), "{:?}", app.input);
        // The old default no longer binds: shift+enter is now inert (the
        // user override replaced the default key set).
        let action = app.handle_key(key_code(KeyCode::Enter, KeyModifiers::SHIFT));
        assert!(action.is_none(), "{action:?}");
        type_str(&mut app, "there");
        let action = app.handle_key(key_code(KeyCode::Enter, KeyModifiers::NONE));
        assert!(is_submit(action.clone(), "hi\nthere"), "{action:?}");
        assert_eq!(app.input, "");
        // Unbound actions (default empty) stay inert.
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "x");
        app.handle_key(key_code(KeyCode::Up, KeyModifiers::CONTROL));
        assert_eq!(app.input, "x", "ctrl+up is previousPrompt, not history, in v0");
    }

    /// WP-609: `/model` without args opens the model picker (bundled
    /// registry labels), `/resume` asks the terminal for the session list.
    #[test]
    fn wp609_pickers_open_from_commands() {
        let mut app = TuiApp::new("m", "cwd", "s");
        assert_eq!(submit_cmd(&mut app, "/model"), None);
        let p = app.picker.as_ref().expect("model picker open");
        assert_eq!(app.picker_kind, PickerKind::Model);
        let items = p.visible();
        assert!(!items.is_empty(), "bundled registry is not empty");
        assert!(items.iter().all(|i| i.display.contains('/')), "{items:?}");
        // First provider alphabetically in the bundled catalog.
        assert!(items.first().is_some_and(|i| i.display.starts_with("amazon-bedrock/")), "{:?}", items.first());
        // The picker renders in the transcript area.
        assert!(app.render_string().contains("Select model"), "picker renders");
        // /resume routes to the terminal (session list load).
        app.picker = None;
        app.input.clear();
        app.cursor = (0, 0);
        assert_eq!(submit_cmd(&mut app, "/resume"), Some(TuiAction::OpenSessionsPicker));
        // /model with args still switches directly (no picker).
        app.input.clear();
        app.cursor = (0, 0);
        assert_eq!(
            submit_cmd(&mut app, "/model fake/echo"),
            Some(TuiAction::SetModel { provider: "fake".into(), model: "echo".into() })
        );
    }

    /// WP-609: picker selection flow — filter, navigate, enter returns the
    /// selection with its value; cancel returns Cancelled.
    #[test]
    fn wp609_picker_selection_flow() {
        use crate::tui::picker::PickerItem;
        let mut app = TuiApp::new("m", "cwd", "s");
        app.picker_kind = PickerKind::Session;
        app.picker = Some(picker::Picker::new(
            "Resume session",
            vec![
                PickerItem { display: "alpha · 2026-01-01T00:00:00Z".into(), value: "/s/a.jsonl".into() },
                PickerItem { display: "beta · 2026-01-02T00:00:00Z".into(), value: "/s/b.jsonl".into() },
            ],
        ));
        // Filter to "be".
        app.handle_picker_key(&TermEvent::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('b'), crossterm::event::KeyModifiers::NONE)));
        assert_eq!(app.picker.as_ref().unwrap().visible().len(), 1);
        // Enter selects the filtered item.
        match app.handle_picker_key(&enter()) {
            Some(picker::PickerOutcome::Selected { value, .. }) => assert_eq!(value, "/s/b.jsonl"),
            other => panic!("expected selection: {other:?}"),
        }
        assert!(app.picker.is_some(), "picker closes via the terminal");
        // Escape cancels.
        app.picker = Some(picker::Picker::new("Resume session", vec![PickerItem { display: "x".into(), value: "y".into() }]));
        assert_eq!(app.handle_picker_key(&TermEvent::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc, crossterm::event::KeyModifiers::NONE))), Some(picker::PickerOutcome::Cancelled));
        // An empty list renders gracefully.
        app.picker = Some(picker::Picker::new("Resume session", vec![]));
        assert!(app.render_string().contains("no matches"), "empty picker renders a placeholder");
    }

    /// WP-608: the slash menu opens on "/" and shows built-ins, the
    /// prompt template and the extension command; a prefix filters it.
    #[test]
    fn wp608_menu_shows_extension_and_template() {
        use crate::templates::Template;
        use crate::tui::commands::Command;
        use std::path::PathBuf;
        let mut app = TuiApp::new("m", "cwd", "s");
        app = app.with_commands(
            vec![Command {
                name: "deploy".into(),
                description: "Deploy the app".into(),
                argument_hint: None,
                source: crate::tui::commands::Source::Extension,
            }],
            vec![Template { name: "fix".into(), content: "fix $1".into(), path: PathBuf::from("prompts/fix.md") }],
        );
        type_str(&mut app, "/");
        let items = app.slash.as_ref().expect("menu open on /").0.clone();
        let names: Vec<&str> = items.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"settings"), "builtins in menu: {names:?}");
        assert!(names.contains(&"fix"), "template in menu: {names:?}");
        assert!(names.contains(&"deploy"), "extension command in menu: {names:?}");
        // The menu renders above the input pane (8 visible rows).
        let rendered = app.render_string();
        assert!(rendered.contains("/settings"), "menu rows rendered: {rendered:?}");
        // A prefix filters to the matching commands — the extension row is
        // then visible (no truncation).
        type_str(&mut app, "de");
        let items = app.slash.as_ref().expect("menu still open").0.clone();
        assert_eq!(items.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["deploy"]);
        assert!(app.render_string().contains("/deploy"), "extension row rendered");
    }

    /// WP-608: the select keys navigate the menu; Enter completes the name
    /// into the editor (one more Enter would submit).
    #[test]
    fn wp608_menu_navigate_and_complete() {
        use crate::tui::commands::Command;
        let mut app = TuiApp::new("m", "cwd", "s");
        app = app.with_commands(
            vec![Command {
                name: "deploy".into(),
                description: "d".into(),
                argument_hint: None,
                source: crate::tui::commands::Source::Extension,
            }],
            vec![],
        );
        type_str(&mut app, "/de");
        assert_eq!(app.slash.as_ref().unwrap().1, 0);
        let down = TermEvent::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_key(down);
        // One item: wraps back to 0 (menu stays open).
        assert_eq!(app.slash.as_ref().unwrap().1, 0);
        app.handle_key(enter());
        assert_eq!(app.input, "/deploy ");
        assert!(app.slash.is_none(), "menu closed after completion");
        // Escape closes the menu without touching the input.
        app.input.clear();
        app.cursor = (0, 0);
        type_str(&mut app, "/se");
        assert!(app.slash.is_some());
        app.handle_key(TermEvent::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )));
        assert!(app.slash.is_none());
        assert_eq!(app.input, "/se");
    }

    /// The upstream submit flow: the first Enter completes the open menu,
    /// the second submits (a closed menu dispatches on the first Enter).
    fn submit_cmd(app: &mut TuiApp, text: &str) -> Option<TuiAction> {
        type_str(app, text);
        match app.handle_key(enter()) {
            Some(a) => Some(a),
            None => app.handle_key(enter()),
        }
    }

    /// WP-608: dispatch — extension command over the bridge path, template
    /// expansion as a normal message, local builtins in place, unknown with
    /// the available list, v0-unwired builtins with a typed error.
    #[test]
    fn wp608_dispatch_paths() {
        use crate::templates::Template;
        use crate::tui::commands::Command;
        use std::path::PathBuf;
        let mut app = TuiApp::new("m", "cwd", "s");
        app = app.with_commands(
            vec![Command {
                name: "deploy".into(),
                description: "d".into(),
                argument_hint: None,
                source: crate::tui::commands::Source::Extension,
            }],
            vec![Template { name: "fix".into(), content: "fix: $@".into(), path: PathBuf::from("prompts/fix.md") }],
        );
        // Extension command: the M1 bridge `command` path.
        assert_eq!(
            submit_cmd(&mut app, "/deploy prod"),
            Some(TuiAction::BridgeCommand { name: "deploy".into(), args: "prod".into() })
        );
        assert_eq!(app.input, "");
        // Template: expanded and submitted as a normal user message.
        assert_eq!(submit_cmd(&mut app, "/fix the bug"), Some(TuiAction::Submit("fix: the bug".into())));
        // Local builtins.
        assert_eq!(submit_cmd(&mut app, "/session"), None);
        assert!(app.transcript.last().unwrap().text.contains("session: s"), "{:?}", app.transcript.last());
        assert_eq!(submit_cmd(&mut app, "/hotkeys"), None);
        assert!(app.transcript.last().unwrap().text.contains("tui.input.submit"));
        // Unknown command: error listing the available ones, input kept.
        assert_eq!(submit_cmd(&mut app, "/zzz"), None);
        let err = app.last_error.clone().expect("error set");
        assert!(err.starts_with("unknown command /zzz"), "{err}");
        assert!(err.contains("/settings") && err.contains("/deploy") && err.contains("/fix"), "{err}");
        assert_eq!(app.input, "/zzz");
        // Known builtin, not wired in v0.
        app.input.clear();
        app.cursor = (0, 0);
        assert_eq!(submit_cmd(&mut app, "/export"), None);
        assert_eq!(app.last_error.as_deref(), Some("/export is not supported in v0"));
        // /quit exits.
        app.last_error = None;
        app.input.clear();
        app.cursor = (0, 0);
        assert_eq!(submit_cmd(&mut app, "/quit"), None);
        assert!(app.exit);
    }

    /// WP-608: the bridge round-trip result renders in the transcript (and
    /// a set_model success updates the status bar label).
    #[test]
    fn wp608_bridge_result_event() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.apply_event(&serde_json::json!({
            "kind": "bridge_cmd_result", "label": "/deploy", "ok": true, "text": "shipped"
        }));
        assert_eq!(app.transcript.last().unwrap().text, "/deploy — shipped");
        app.apply_event(&serde_json::json!({
            "kind": "bridge_cmd_result", "label": "/model", "ok": false, "text": "unknown model x/y"
        }));
        assert_eq!(app.transcript.last().unwrap().text, "/model: unknown model x/y");
        app.apply_event(&serde_json::json!({
            "kind": "bridge_cmd_result", "label": "/model", "ok": true, "text": "", "model": "openai/gpt-x"
        }));
        assert_eq!(app.transcript.last().unwrap().text, "/model ok");
        assert_eq!(app.model, "openai/gpt-x");
    }

    /// WP-607 golden: each built-in theme renders the fixture screen with
    /// its own token colors (buffer snapshot over the real render path).
    #[test]
    fn wp607_builtin_themes_render_golden_colors() {
        use ratatui::style::Color;
        // (theme, mdHeading, mdListBullet, userMessageText)
        let cases: &[(&str, Color, Color, Color)] = &[
            ("dark", Color::Rgb(205, 154, 34), Color::Rgb(167, 152, 215), Color::Rgb(222, 224, 225)),
            ("light", Color::Rgb(143, 104, 2), Color::Rgb(116, 89, 180), Color::Rgb(59, 63, 65)),
        ];
        for (name, heading, bullet, user_text) in cases {
            let theme = keybindings_dummy_theme(name);
            let mut app = TuiApp::new("m", "cwd", "s");
            app.theme = theme;
            app.transcript.push(Msg {
                role: "assistant",
                text: "# Head\n- item\nsee [docs](https://x)".to_string(),
            });
            app.transcript.push(Msg { role: "user", text: "hello".to_string() });
            let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, app.size.0, app.size.1));
            app.render_to(&mut buf, Rect::new(0, 0, app.size.0, app.size.1));
            let cell = |needle: &str| -> Option<ratatui::buffer::Cell> {
                buf.content.iter().find(|c| c.symbol() == needle).cloned()
            };
            let h = cell("H").unwrap_or_else(|| panic!("{name}: heading not rendered"));
            assert_eq!(h.fg, *heading, "{name}: heading color");
            let b = cell("-").unwrap_or_else(|| panic!("{name}: bullet not rendered"));
            assert_eq!(b.fg, *bullet, "{name}: bullet color");
            let p = cell("\u{25b8}").unwrap_or_else(|| panic!("{name}: user prefix not rendered"));
            assert_eq!(p.fg, *user_text, "{name}: user prefix color");
            // The status bar uses the theme's muted token.
            // The status bar (last row) uses the theme's muted token.
            let last_row = (app.size.1 as usize - 1) * app.size.0 as usize;
            let s_cell = &buf.content[last_row];
            assert_ne!(s_cell.symbol(), " ", "{name}: status bar not rendered");
            let muted = app.theme.color("muted");
            assert_eq!(s_cell.fg, muted, "{name}: status bar color");
        }
    }

    fn keybindings_dummy_theme(name: &str) -> theme::Theme {
        theme::Theme::builtin(name).expect("built-in theme")
    }

    /// WP-607: `/theme <name>` submits the SwitchTheme action; an unknown
    /// name is rejected by the loader with the available themes listed.
    #[test]
    fn wp606_theme_command_switches() {
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "/theme light");
        // WP-608: the slash menu is open while a bare name is typed — the
        // first Enter completes it, the second submits.
        assert_eq!(submit_cmd(&mut app, ""), Some(TuiAction::SwitchTheme("light".into())));
        assert_eq!(app.input, "");
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "/theme");
        assert_eq!(submit_cmd(&mut app, ""), Some(TuiAction::SwitchTheme(" ".trim().to_string())));
        // The loader rejects unknown themes with the available list.
        let dir = std::env::temp_dir().join(format!("pi-rust-theme-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = theme::Theme::load(&dir, "nope").unwrap_err();
        assert!(err.contains("nope") && err.contains("dark"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// WP-607: the settings `theme` value resolves per upstream semantics.
    #[test]
    fn wp607_theme_setting_resolves() {
        assert_eq!(theme::resolve_theme_setting(Some("light")), Some("light".into()));
        assert_eq!(theme::resolve_theme_setting(Some("a/b")), Some("b".into()));
        assert_eq!(theme::resolve_theme_setting(Some("a/b/c")), None);
    }

    fn is_submit(action: Option<TuiAction>, text: &str) -> bool {
        matches!(&action, Some(TuiAction::Submit(t)) if t == text)
    }
    fn editor_outcome(action: Option<TuiAction>) -> Option<dialog::DialogOutcome> {
        match action {
            Some(TuiAction::EditorClosed(o)) => Some(o),
            _ => None,
        }
    }

    #[test]
    fn wp605_ctrl_d_and_ctrl_c_still_exit_when_empty() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.handle_key(key_code(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(app.exit, "ctrl+d with empty input exits");
        let mut app = TuiApp::new("m", "cwd", "s");
        type_str(&mut app, "x");
        app.handle_key(key_code(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(app.input, "", "ctrl+c clears");
        app.handle_key(key_code(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.exit, "second ctrl+c exits");
    }

    #[test]
    fn wp605_multiline_renders_in_input_area() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.handle_key(paste("alpha\nbeta"));
        let s = app.render_string();
        assert!(s.contains("alpha"), "{s}");
        assert!(s.contains("beta"), "continuation line visible: {s}");
    }

    #[test]
    fn dialog_owns_keys_until_it_closes() {
        let mut app = TuiApp::new("m", "cwd", "s");
        let req = json!({"id":"d1","method":"select","title":"Pick","options":["a","b"],"timeout":60000});
        assert!(app.open_dialog(&req));
        // Typed characters do not leak into the editor.
        let _ = app.handle_key(key('x'));
        assert_eq!(app.input, "");
        // The dialog gets the keys: down + enter answers "b".
        let _ = app.handle_dialog_key(&key_code(KeyCode::Down, KeyModifiers::NONE));
        let Some(outcome) = app.handle_dialog_key(&key_code(KeyCode::Enter, KeyModifiers::NONE)) else {
            panic!("expected dialog answer");
        };
        assert!(app.dialog.is_none(), "dialog must close after answering");
        match outcome {
            dialog::DialogOutcome::Answered { response, .. } => {
                assert_eq!(response["value"], json!("b"));
                assert_eq!(response["id"], json!("d1"));
            }
            other => panic!("unexpected outcome {other:?}"),
        }
        // After the dialog, editor keys work again.
        let _ = app.handle_key(key('h'));
        assert_eq!(app.input, "h");
    }

    #[test]
    fn expired_dialog_closes_without_response() {
        let mut app = TuiApp::new("m", "cwd", "s");
        let req = json!({"id":"d2","method":"confirm","title":"t","message":"m","timeout":1});
        app.open_dialog(&req);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let outcome = app.take_expired_dialog(std::time::Instant::now()).expect("expired");
        assert!(app.dialog.is_none());
        match outcome {
            dialog::DialogOutcome::TimedOut { id, method, .. } => {
                assert_eq!(id, "d2");
                assert_eq!(method, "confirm");
            }
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[test]
    fn ff_events_render_in_tui() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.apply_event(&json!({"kind":"notify","message":"hello"}));
        assert!(app.transcript.last().unwrap().text.contains("hello"));
        app.apply_event(&json!({"kind":"set_status","statusKey":"k","statusText":"busy"}));
        assert_eq!(app.status_text.as_deref(), Some("busy"));
        app.apply_event(&json!({"kind":"set_title","title":"MyTitle"}));
        assert_eq!(app.title.as_deref(), Some("MyTitle"));
        app.apply_event(&json!({"kind":"set_editor_text","text":"preset"}));
        assert_eq!(app.input, "preset");
        let s = app.render_string();
        assert!(s.contains("MyTitle"), "title in status bar:\n{s}");
        assert!(s.contains("busy"), "status text in status bar:\n{s}");
    }

    #[test]
    fn dialog_renders_in_transcript_area() {
        let mut app = TuiApp::new("m", "cwd", "s");
        app.open_dialog(&json!({"id":"d3","method":"select","title":"PickOne","options":["alpha","beta"],"timeout":60000}));
        let s = app.render_string();
        assert!(s.contains("PickOne"), "{s}");
        assert!(s.contains("alpha"), "{s}");
        assert!(s.contains('▸'), "cursor marker: {s}");
    }

    fn enter() -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }
    fn ctrl_c() -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
    }
    fn ctrl_d() -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
    }
    fn resize(w: u16, h: u16) -> TermEvent {
        TermEvent::Resize(w, h)
    }

    #[test]
    fn types_submits_and_clears_input() {
        let mut app = TuiApp::new("fake/fake", "/w", "s1");
        for c in "hi".chars() {
            assert_eq!(app.handle_key(key(c)), None);
        }
        assert_eq!(app.input, "hi");
        assert_eq!(app.handle_key(enter()), Some(TuiAction::Submit("hi".into())));
        assert_eq!(app.input, "");
        assert!(!app.exit);
    }

    #[test]
    fn backspace_and_empty_submit_noop() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.handle_key(key('a'));
        app.handle_key(key('b'));
        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.input, "a");
        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.input, "");
        assert_eq!(app.handle_key(enter()), None, "empty submit yields nothing");
    }

    #[test]
    fn q_is_a_plain_character() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.handle_key(key('q'));
        assert_eq!(app.input, "q", "upstream has no bare-q exit; WP-606 formalizes bindings");
        assert!(!app.exit);
    }

    #[test]
    fn ctrl_c_clears_input_then_exits_when_empty() {
        // Upstream app.clear: ctrl+c clears the editor; with the editor
        // already empty it exits ("ctrl+c twice to exit").
        let mut app = TuiApp::new("m", "/w", "s");
        app.handle_key(key('a'));
        app.handle_key(ctrl_c());
        assert_eq!(app.input, "", "first ctrl+c clears");
        assert!(!app.exit);
        app.handle_key(ctrl_c());
        assert!(app.exit, "second ctrl+c (empty editor) exits");
    }

    #[test]
    fn ctrl_d_exits_only_with_empty_input() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.handle_key(key('x'));
        app.handle_key(ctrl_d());
        assert!(!app.exit, "ctrl+d with text does not exit (upstream app.exit)");
        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        app.handle_key(ctrl_d());
        assert!(app.exit, "ctrl+d with empty editor exits");
    }

    #[test]
    fn events_fill_transcript_and_pending_flag() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"user_input","text":"hello"}));
        app.apply_event(&json!({"kind":"agent_start"}));
        assert!(app.pending);
        app.apply_event(&json!({"kind":"tool_execution","name":"bash","blocked":false,"result_summary":"out"}));
        app.apply_event(&json!({"kind":"model_output","text":"echo: hello"}));
        app.apply_event(&json!({"kind":"agent_end"}));
        assert!(!app.pending);

        let screen = app.render_string();
        assert!(screen.contains("▸ hello"), "user line: {screen}");
        assert!(screen.contains("bash: out"), "tool line: {screen}");
        assert!(screen.contains("echo: hello"), "assistant line: {screen}");
    }

    #[test]
    fn blocked_tool_marks_blocked() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"tool_execution","name":"bash","blocked":true,"result_summary":"Dangerous command blocked"}));
        assert!(app.render_string().contains("blocked"));
    }

    #[test]
    fn pending_indicator_shows_only_while_working() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"user_input","text":"x"}));
        assert!(!app.render_string().contains("● working"));
        app.apply_event(&json!({"kind":"agent_start"}));
        assert!(app.render_string().contains("● working"), "status bar shows working");
        app.apply_event(&json!({"kind":"agent_end"}));
        assert!(!app.render_string().contains("● working"));
    }

    #[test]
    fn resize_updates_size_and_renders_without_panic() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"model_output","text":"line"}));
        app.handle_key(resize(100, 30));
        assert_eq!(app.size, (100, 30));
        let _ = app.render_string();
        app.handle_key(resize(0, 0));
        assert_eq!(app.size, (100, 30), "zero size ignored");
    }

    #[test]
    fn status_bar_truncates_with_ellipsis() {
        let mut app = TuiApp::new("m", "/a/very/long/cwd/path/that/goes/on/and/on", "s".repeat(50));
        let screen = app.render_string();
        let last = screen.lines().last().unwrap();
        assert!(last.chars().count() <= 80, "status row within width: {last}");
        assert!(last.ends_with('…'), "truncated with ellipsis: {last}");
    }

    #[test]
    fn integration_smoke_send_reply_exit() {
        // WP-601 acceptance: enter TUI, send a message, the (scripted fake)
        // model reply is rendered, q exits cleanly.
        let mut app = TuiApp::new("fake/fake", "/repo", "session-1");
        assert_eq!(app.handle_key(resize(80, 24)), None);

        for c in "hello".chars() {
            app.handle_key(key(c));
        }
        assert_eq!(app.handle_key(enter()), Some(TuiAction::Submit("hello".into())));

        // What the core would emit for a fake-echo run:
        app.apply_event(&json!({"kind":"user_input","index":0,"source":"tui","text":"hello"}));
        app.apply_event(&json!({"kind":"agent_start"}));
        app.apply_event(&json!({"kind":"turn_start","turnIndex":0}));
        app.apply_event(&json!({"kind":"model_output","turnIndex":0,"text":"echo: hello"}));
        app.apply_event(&json!({"kind":"turn_end","turnIndex":0}));
        app.apply_event(&json!({"kind":"agent_end"}));

        let screen = app.render_string();
        assert!(screen.contains("▸ hello"), "{screen}");
        assert!(screen.contains("echo: hello"), "{screen}");
        assert!(screen.contains("fake/fake"), "status bar model: {screen}");

        app.handle_key(ctrl_d());
        assert!(app.exit);
    }

    #[test]
    fn model_delta_appends_to_assistant_message() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"user_input","text":"hi"}));
        app.apply_event(&json!({"kind":"model_delta","turnIndex":0,"text":"Hel"}));
        app.apply_event(&json!({"kind":"model_delta","turnIndex":0,"text":"lo"}));
        let screen = app.render_string();
        assert_eq!(app.transcript.len(), 2, "one user + one streaming assistant msg");
        assert!(app.transcript[1].role == "assistant" && app.transcript[1].text == "Hello",
            "{:?}", app.transcript);
        assert!(screen.contains("Hello"), "{screen}");
    }

    #[test]
    fn model_output_after_stream_does_not_duplicate() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"model_delta","text":"echo: hi"}));
        app.apply_event(&json!({"kind":"model_output","text":"echo: hi"}));
        assert_eq!(app.transcript.len(), 1, "streamed text is not repeated: {:?}", app.transcript);
        // Non-streaming driver (no deltas before): the full text is pushed.
        let mut app2 = TuiApp::new("m", "/w", "s");
        app2.apply_event(&json!({"kind":"model_output","text":"echo: hi"}));
        assert_eq!(app2.transcript.len(), 1);
        assert_eq!(app2.transcript[0].text, "echo: hi");
    }

    #[test]
    fn first_token_visible_under_100ms() {
        // In-process pipeline: broadcast delta -> TUI apply -> render.
        // (The PTY/e2e path adds no extra hops; WP-603 acceptance < 100ms.)
        let mut app = TuiApp::new("m", "/w", "s");
        let t0 = std::time::Instant::now();
        app.apply_event(&json!({"kind":"model_delta","turnIndex":0,"text":"first"}));
        let screen = app.render_string();
        let elapsed = t0.elapsed();
        assert!(screen.contains("first"), "{screen}");
        assert!(elapsed < std::time::Duration::from_millis(100),
            "first token render took {elapsed:?}");
    }

    #[test]
    fn hundred_token_stream_frame_budget() {
        // 100 deltas, worst case: one full render per token (coalescing only
        // helps). The 100-token stream must stay far inside a 60fps frame
        // budget total.
        let mut app = TuiApp::new("m", "/w", "s");
        let t0 = std::time::Instant::now();
        for i in 0..100 {
            app.apply_event(&json!({"kind":"model_delta","turnIndex":0,"text":format!("tok{i} ")}));
            let _ = app.render_string();
        }
        let total = t0.elapsed();
        let screen = app.render_string();
        assert!(screen.contains("tok0"), "first token kept: {screen}");
        assert!(screen.contains("tok99"), "last token present: {screen}");
        assert!(total < std::time::Duration::from_secs(1),
            "100-token stream rendered in {total:?}");
    }

    #[test]
    fn markdown_renders_in_pane() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.apply_event(&json!({"kind":"model_output","text":"# Plan\n- do `x` **now**\n```sh\necho hi\n```"}));
        let screen = app.render_string();
        assert!(screen.contains("1 Plan"), "heading in pane: {screen}");
        assert!(screen.contains("- do `x` now"), "list+inline in pane: {screen}");
        assert!(screen.contains("sh"), "code label in pane: {screen}");
        assert!(screen.contains("echo hi"), "code line in pane: {screen}");
    }

    #[test]
    fn long_code_block_scrolls_within_pane() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.size = (20, 6); // tiny pane: 3 transcript rows
        let code: String = (0..40).map(|i| format!("line-{i}")).collect::<Vec<_>>().join("\n");
        app.apply_event(&json!({"kind":"model_output","text":format!("```\n{code}\n```")}));
        let screen = app.render_string();
        assert!(!screen.contains("line-0"), "oldest code scrolled away: {screen}");
        assert!(screen.contains("line-39"), "newest code pinned: {screen}");
        assert!(!screen.contains("line-20"), "middle scrolled away: {screen}");
    }

    #[test]
    fn long_assistant_line_wraps() {
        let mut app = TuiApp::new("m", "/w", "s");
        app.size = (20, 6);
        app.apply_event(&json!({"kind":"model_output","text":"aaaa bbbb cccc dddd eeee"}));
        let screen = app.render_string();
        let rows: Vec<&str> = screen.lines().collect();
        assert!(rows.iter().any(|r| r.contains("aaaa bbbb")), "first wrap row: {screen}");
        assert!(rows.iter().any(|r| r.contains("cccc")), "wrapped continuation: {screen}");
        assert!(rows.iter().all(|r| r.chars().count() <= 20), "no row overflows width: {screen}");
    }

    #[test]
    fn long_transcript_scrolls_and_pins_to_bottom() {
        let mut app = TuiApp::new("m", "/w", "s");
        for i in 0..60 {
            app.apply_event(&json!({"kind":"model_output","text":format!("line-{i}")}));
        }
        let screen = app.render_string();
        assert!(!screen.contains("line-0"), "oldest line scrolled away");
        assert!(screen.contains("line-59"), "newest line pinned: {screen}");

        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        app.handle_key(TermEvent::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        let scrolled = app.render_string();
        assert!(!scrolled.contains("line-59"), "scrolling up moves the window: {scrolled}");
    }

    #[test]
    fn wp705_extension_shortcut_dispatches_before_builtins() {
        let mut app = TuiApp::new("m", "/w", "s");
        app = app.with_shortcuts(vec![(
            "ctrl+shift+x".to_string(),
            "fixture action".to_string(),
        )]);
        // A registered extension shortcut dispatches over the bridge and
        // consumes the key (upstream custom-editor.handleInput checks
        // extension shortcuts before any built-in binding).
        let action = app.handle_key(key_mod('x', KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert_eq!(
            action,
            Some(TuiAction::BridgeShortcut {
                key: "ctrl+shift+x".to_string()
            })
        );
        assert_eq!(app.input, "", "the matched key is consumed");
        // A key without an extension shortcut still reaches the built-ins.
        assert_eq!(app.handle_key(key('h')), None);
        assert_eq!(app.input, "h");
    }
}
