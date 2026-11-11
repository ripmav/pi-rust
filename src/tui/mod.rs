//! WP-601: TUI skeleton (ratatui). The app state, key handling, and rendering
//! live here and are fully in-process testable (key injection + buffer
//! snapshots, no TTY). The terminal adapter (crossterm) is in `terminal`.
//!
//! Terminal-craft floor: one accent role, dense chrome, single-row status
//! bar, no boxes-in-boxes, `…` truncation in the status bar.
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::buffer::Buffer;
use serde_json::Value;

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
    pub input: String,
    /// Model is working (a turn is in flight).
    pub pending: bool,
    pub model: String,
    pub cwd: String,
    pub session: String,
    /// Lines scrolled up from the bottom (0 = pinned to newest).
    pub scroll: usize,
    pub size: (u16, u16),
    pub exit: bool,
    pub last_error: Option<String>,
}

impl TuiApp {
    pub fn new(model: impl Into<String>, cwd: impl Into<String>, session: impl Into<String>) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            pending: false,
            model: model.into(),
            cwd: cwd.into(),
            session: session.into(),
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

    /// Handle one terminal event. Returns the submitted text on Enter.
    pub fn handle_key(&mut self, ev: TermEvent) -> Option<String> {
        match ev {
            TermEvent::Resize(w, h) => {
                self.set_size(w, h);
                None
            }
            TermEvent::Key(k) => {
                if k.modifiers.contains(KeyModifiers::CONTROL) {
                    match k.code {
                        // Upstream app.clear: clears the input; a second press
                        // with the input already empty exits (WP-606 rebinds).
                        KeyCode::Char('c') => {
                            if self.input.is_empty() {
                                self.exit = true;
                            } else {
                                self.input.clear();
                            }
                            return None;
                        }
                        // Upstream app.exit: exit when the editor is empty.
                        KeyCode::Char('d') if self.input.is_empty() => {
                            self.exit = true;
                            return None;
                        }
                        _ => return None,
                    }
                }
                match k.code {
                    KeyCode::Enter => {
                        if k.modifiers.contains(KeyModifiers::SHIFT) {
                            // shift+enter: newline (WP-605 refines editor behavior).
                            self.input.push('\n');
                        } else {
                            let t = self.input.trim().to_string();
                            self.input.clear();
                            if !t.is_empty() {
                                return Some(t);
                            }
                        }
                        None
                    }
                    KeyCode::Char(c) => {
                        self.input.push(c);
                        self.scroll = 0;
                        None
                    }
                    KeyCode::Backspace => {
                        self.input.pop();
                        None
                    }
                    // WP-605: history, word editing; WP-606: rebindable table.
                    KeyCode::Up if self.input.is_empty() => {
                        self.scroll += 1;
                        None
                    }
                    KeyCode::Down if self.input.is_empty() => {
                        self.scroll = self.scroll.saturating_sub(1);
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
                self.transcript.push(Msg { role: "assistant", text });
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
            _ => {} // decision / extension_ui_request: WP-604 dialogs.
        }
    }

    /// Render into the given buffer at the given area (in-process testable).
    pub fn render_to(&self, buf: &mut Buffer, area: Rect) {
        let (w, h) = (area.width as usize, area.height as usize);
        let status_h = 1usize.min(h);
        let input_h = 1usize.min(h.saturating_sub(status_h));
        let transcript_h = h.saturating_sub(status_h).saturating_sub(input_h);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(transcript_h as u16),
                Constraint::Length(input_h as u16),
                Constraint::Length(status_h as u16),
            ])
            .split(area);

        // Transcript: role-marked, wrapped, scrolled from the bottom.
        let mut lines: Vec<Line> = Vec::new();
        for m in &self.transcript {
            let (prefix, style): (&str, Style) = match m.role {
                "user" => ("▸ ", Style::default().add_modifier(Modifier::BOLD)),
                "assistant" => ("", Style::default()),
                "tool" => ("⚙ ", Style::default().add_modifier(Modifier::DIM)),
                _ => ("· ", Style::default().add_modifier(Modifier::DIM)),
            };
            for (i, seg) in m.text.split('\n').enumerate() {
                lines.push(Line::from(vec![
                    if i == 0 { Span::styled(prefix, style.clone()) } else { Span::raw("") },
                    Span::styled(seg.to_string(), style.clone()),
                ]));
            }
        }
        if self.pending && lines.last().and_then(|l| l.spans.last()).map(|s| s.content.as_ref().is_empty()).unwrap_or(true) {
            lines.push(Line::from(Span::styled("● ", Style::default().add_modifier(Modifier::DIM))));
        }
        let total = lines.len();
        let start = total.saturating_sub(transcript_h).saturating_sub(self.scroll);
        let visible: Vec<Line> = lines[start..total.min(start + transcript_h)].to_vec();
        Paragraph::new(visible).render(chunks[0], buf);

        // Input line.
        if input_h > 0 {
            let prompt = Span::styled("> ", Style::default().add_modifier(Modifier::BOLD));
            let caret = Span::styled("▌", Style::default().add_modifier(Modifier::BOLD));
            let content: Vec<Span> = vec![prompt, Span::raw(self.input.as_str()), caret.clone()];
            let mut l = Line::from(content);
            if l.width() > w {
                l = Line::from(vec![
                    Span::raw(&self.input[self.input.len().saturating_sub(w.saturating_sub(2))..]),
                    caret,
                ]);
            }
            Paragraph::new(l).render(chunks[1], buf);
        }

        // Status bar: model · cwd · session (+ pending + last error), `…`
        // truncated, one row.
        if status_h > 0 {
            let mut segs: Vec<String> = vec![self.model.clone(), self.cwd.clone(), self.session.clone()];
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
    use crossterm::event::{KeyEvent, KeyModifiers};
    use serde_json::json;

    fn key(c: char) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
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
        assert_eq!(app.handle_key(enter()), Some("hi".to_string()));
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
        assert_eq!(app.handle_key(enter()), Some("hello".to_string()));

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
