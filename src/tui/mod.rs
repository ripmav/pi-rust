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
pub mod markdown;
pub mod terminal;

use crossterm::event::{Event as TermEvent, KeyCode, KeyModifiers};

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
}

impl TuiApp {
    pub fn new(model: impl Into<String>, cwd: impl Into<String>, session: impl Into<String>) -> Self {
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
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                let alt = k.modifiers.contains(KeyModifiers::ALT);
                if ctrl {
                    match k.code {
                        KeyCode::Char('c') => {
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
                        KeyCode::Char('d') if self.editor_session.is_some() => {
                            self.delete_char_forward();
                            return None;
                        }
                        KeyCode::Char('d') if self.input.trim().is_empty() => {
                            self.exit = true;
                            return None;
                        }
                        KeyCode::Char('w') => {
                            self.delete_word_backward();
                            return None;
                        }
                        KeyCode::Char('u') => {
                            self.delete_to_line_start();
                            return None;
                        }
                        KeyCode::Char('k') => {
                            self.delete_to_line_end();
                            return None;
                        }
                        KeyCode::Char('j') => {
                            self.insert_newline();
                            return None;
                        }
                        _ => return None,
                    }
                }
                if alt {
                    match k.code {
                        KeyCode::Backspace => {
                            self.delete_word_backward();
                            return None;
                        }
                        KeyCode::Char('d') => {
                            self.delete_word_forward();
                            return None;
                        }
                        _ => return None,
                    }
                }
                match k.code {
                    KeyCode::Enter => {
                        if k.modifiers.contains(KeyModifiers::SHIFT) {
                            self.insert_newline();
                            return None;
                        }
                        if self.editor_session.is_some() {
                            let value = self.input.clone();
                            let id = self.editor_session.as_ref().unwrap().id.clone();
                            let response = json!({"type":"extension_ui_response","id":id,"value":value});
                            return Some(self.close_editor_session(response));
                        }
                        let t = self.submit_text();
                        t.map(TuiAction::Submit).inspect(|_| self.scroll = 0)
                    }
                    KeyCode::Esc => {
                        if let Some(id) = self.editor_session.as_ref().map(|e| e.id.clone()) {
                            return Some(self.close_editor_session(Self::cancel_response(&id)));
                        }
                        None
                    }
                    KeyCode::Backspace => {
                        self.backspace_at();
                        None
                    }
                    KeyCode::Delete => {
                        self.delete_char_forward();
                        None
                    }
                    KeyCode::Left => {
                        self.move_cursor_left();
                        None
                    }
                    KeyCode::Right => {
                        self.move_cursor_right();
                        None
                    }
                    KeyCode::Home => {
                        self.move_cursor_home();
                        None
                    }
                    KeyCode::End => {
                        self.move_cursor_end();
                        None
                    }
                    KeyCode::Up => {
                        if self.editor_session.is_some() {
                            return None;
                        }
                        if !self.history_prev() {
                            self.scroll += 1;
                        }
                        None
                    }
                    KeyCode::Down => {
                        if self.editor_session.is_some() {
                            return None;
                        }
                        if !self.history_next() {
                            self.scroll = self.scroll.saturating_sub(1);
                        }
                        None
                    }
                    KeyCode::Char(c) => {
                        self.insert_char_at(c);
                        self.scroll = 0;
                        None
                    }
                    _ => None,
                }
            }
            _ => None,
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
        let input_h = (input_rows + editor_chrome).min(h.saturating_sub(status_h).max(1));
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
        if let Some(d) = &self.dialog {
            lines.push(Line::from(Span::raw("")));
            lines.extend(d.render_lines());
        } else {
            for m in &self.transcript {
                let (prefix, style): (&str, Style) = match m.role {
                    "user" => ("▸ ", Style::default().add_modifier(Modifier::BOLD)),
                    "assistant" => ("", Style::default()),
                    "tool" => ("⚙ ", Style::default().add_modifier(Modifier::DIM)),
                    _ => ("· ", Style::default().add_modifier(Modifier::DIM)),
                };
                let body: Vec<Line> = if matches!(m.role, "user" | "assistant") {
                    markdown::md_lines(&m.text)
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
            lines.push(Line::from(Span::styled("● ", Style::default().add_modifier(Modifier::DIM))));
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
            let bold = Style::default().add_modifier(Modifier::BOLD);
            let dim = Style::default().add_modifier(Modifier::DIM);
            let mut pane: Vec<Line> = Vec::new();
            if let Some(es) = &self.editor_session {
                pane.push(Line::from(Span::styled(es.title.clone(), bold.clone())));
            }
            let hint_rows = if self.editor_session.is_some() { 2 } else { 0 };
            let visible = input_h.saturating_sub(hint_rows);
            let text_lines: Vec<String> = self.input.split('\n').map(String::from).collect();
            for (i, line) in text_lines.iter().take(visible).enumerate() {
                let prompt = if i == 0 { "> " } else { "  " };
                let mut spans = vec![Span::styled(prompt.to_string(), bold.clone())];
                if i == self.cursor.0 {
                    let chars: Vec<char> = line.chars().collect();
                    let c = self.cursor.1.min(chars.len());
                    spans.push(Span::raw(chars[..c].iter().collect::<String>()));
                    spans.push(Span::styled("\u{258c}".to_string(), bold.clone()));
                    spans.push(Span::raw(chars[c..].iter().collect::<String>()));
                } else {
                    spans.push(Span::raw(line.clone()));
                }
                pane.push(Line::from(spans));
            }
            if self.editor_session.is_some() {
                pane.push(Line::from(Span::styled(
                    "enter ok \u{b7} esc cancel",
                    dim,
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
            Paragraph::new(Line::from(Span::styled(s, Style::default().fg(Color::Gray))))
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
}
