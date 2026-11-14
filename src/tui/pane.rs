//! WP-1006: the `ctx.ui.custom` descriptor pane (structured subset,
//! `roadmap/m7-extension-completeness/wp-707-ui-custom-eval.md`).
//!
//! The sidecar runs the extension's real pi-tui component and relays
//! `pane_open` / `pane_update` / `pane_close`; the TUI renders the
//! descriptor (title, lines, list with cursor, input) above the editor and
//! relays keys back as `pane_key`. The component decides when the pane
//! closes (`done(result)` → `pane_close`) — the TUI never closes it itself.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{json, Value};

use crossterm::event::{Event as TermEvent, KeyCode};

/// A structured descriptor snapshot from the sidecar (pane_update payload).
#[derive(Debug, Clone, Default)]
pub struct Pane {
    pub id: String,
    pub title: Option<String>,
    pub lines: Vec<String>,
    pub list: Option<PaneList>,
    pub input: Option<PaneInput>,
    /// `blocked: {kind, message}` — shown in the status line while the pane
    /// blocks a program status (upstream `options.blocked`).
    pub blocked: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct PaneList {
    pub items: Vec<String>,
    pub cursor: usize,
}

#[derive(Debug, Clone, Default)]
pub struct PaneInput {
    pub label: String,
    pub value: String,
    pub placeholder: Option<String>,
}

impl Pane {
    /// Build from a `pane_open` event (id + optional blocked; the first
    /// `pane_update` fills the content).
    pub fn from_open(ev: &Value) -> Option<Self> {
        let id = ev.get("id")?.as_str()?.to_string();
        Some(Self {
            id,
            blocked: ev.get("blocked").cloned(),
            ..Default::default()
        })
    }

    /// Merge a `pane_update` payload into the pane (all fields are
    /// replacement, not patch: the descriptor is a full snapshot).
    pub fn apply_update(&mut self, ev: &Value) {
        if ev.get("id").and_then(Value::as_str) != Some(self.id.as_str()) {
            return;
        }
        self.title = ev.get("title").and_then(Value::as_str).map(str::to_string);
        let mut lines = Vec::new();
        if let Some(arr) = ev.get("lines").and_then(Value::as_array) {
            for line in arr {
                if let Some(text) = line.get("text").and_then(Value::as_str) {
                    lines.push(text.to_string());
                }
            }
        }
        self.lines = lines;
        self.list = ev.get("list").map(|l| PaneList {
            items: l
                .get("items")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            cursor: l.get("cursor").and_then(Value::as_u64).unwrap_or(0) as usize,
        });
        self.input = ev.get("input").map(|i| PaneInput {
            label: i
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            value: i
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            placeholder: i
                .get("placeholder")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    /// The `pane_key` wire value for one terminal key, or `None` when the
    /// key is not relayable (modifiers other than none, etc.).
    pub fn key_event(ev: &TermEvent) -> Option<Value> {
        let TermEvent::Key(key) = ev else {
            return None;
        };
        let (code, mods) = (key.code, key.modifiers);
        if mods != crossterm::event::KeyModifiers::NONE {
            return None;
        }
        let key = match code {
            KeyCode::Up => json!({"code": "up"}),
            KeyCode::Down => json!({"code": "down"}),
            KeyCode::Left => json!({"code": "left"}),
            KeyCode::Right => json!({"code": "right"}),
            KeyCode::Enter => json!({"code": "enter"}),
            KeyCode::Esc => json!({"code": "escape"}),
            KeyCode::Backspace => json!({"code": "backspace"}),
            KeyCode::Tab => json!({"code": "tab"}),
            KeyCode::Char(c) => json!({"code": "char", "text": c.to_string()}),
            _ => return None,
        };
        Some(key)
    }

    /// Render the descriptor (title + lines + list + input). `width` is the
    /// render width; list rows are clipped to it.
    pub fn render_lines(&self, width: u16) -> Vec<Line<'static>> {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let muted = Style::default().fg(Color::DarkGray);
        let dim = Style::default().fg(Color::Gray);
        let mut out = Vec::new();
        if let Some(title) = &self.title {
            out.push(Line::from(Span::styled(title.clone(), bold)));
        }
        for text in &self.lines {
            out.push(Line::from(Span::styled(text.clone(), muted)));
        }
        if let Some(list) = &self.list {
            for (i, item) in list.items.iter().enumerate() {
                let marker = if i == list.cursor { "\u{25b8} " } else { "  " };
                let text = format!("{marker}{item}");
                let text = if text.chars().count() > width as usize {
                    // Clip on char boundaries (ANSI-free descriptor text).
                    text.chars().take(width as usize).collect::<String>()
                } else {
                    text
                };
                out.push(Line::from(Span::styled(
                    text,
                    if i == list.cursor { bold } else { dim },
                )));
            }
        }
        if let Some(input) = &self.input {
            let shown = if input.value.is_empty() {
                input.placeholder.clone().unwrap_or_default()
            } else {
                input.value.clone()
            };
            out.push(Line::from(Span::styled(
                format!("{} {shown}", input.label),
                Style::default(),
            )));
        }
        if let Some(blocked) = &self.blocked {
            // Upstream `options.blocked`: the pane blocks a program status —
            // show the message.
            let msg = blocked
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("working\u{2026}");
            out.push(Line::from(Span::styled(msg.to_string(), dim)));
        }
        if out.is_empty() {
            out.push(Line::from(Span::styled("\u{2026}".to_string(), muted)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_open_and_update() {
        let mut pane = Pane::from_open(
            &json!({"type":"pane_open","id":"pane_1","blocked":{"kind":"ui","message":"working"}}),
        )
        .unwrap();
        assert_eq!(pane.id, "pane_1");
        assert!(pane.blocked.is_some());
        pane.apply_update(&json!({
            "type": "pane_update", "id": "pane_1", "title": "MCP",
            "lines": [{"text": "Manage servers"}],
            "list": {"items": ["a", "b"], "cursor": 1},
            "input": {"label": "> ", "value": "x", "placeholder": "ph"}
        }));
        assert_eq!(pane.title.as_deref(), Some("MCP"));
        assert_eq!(pane.list.as_ref().unwrap().cursor, 1);
        assert_eq!(pane.input.as_ref().unwrap().value, "x");
        // A stale id is ignored.
        pane.apply_update(&json!({"type":"pane_update","id":"pane_2","title":"nope"}));
        assert_eq!(pane.title.as_deref(), Some("MCP"));
    }

    #[test]
    fn key_encoding() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mk = |code: KeyCode| TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(
            Pane::key_event(&mk(KeyCode::Up)),
            Some(json!({"code":"up"}))
        );
        assert_eq!(
            Pane::key_event(&mk(KeyCode::Char('a'))),
            Some(json!({"code":"char","text":"a"}))
        );
        assert_eq!(Pane::key_event(&mk(KeyCode::PageUp)), None);
    }

    #[test]
    fn render_marks_cursor_row() {
        let mut pane = Pane::from_open(&json!({"id":"p"})).unwrap();
        pane.apply_update(&json!({"id":"p","title":"T","list":{"items":["one","two"],"cursor":1}}));
        let lines = pane.render_lines(80);
        let has = |needle: &str| {
            lines.iter().any(|l| {
                l.spans
                    .iter()
                    .any(|sp| sp.content.as_ref().contains(needle))
            })
        };
        assert!(has("\u{25b8} two"), "{lines:?}");
        assert!(has("  one"), "{lines:?}");
    }
}
