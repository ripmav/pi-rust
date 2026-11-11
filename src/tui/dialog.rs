//! WP-604: human-in-the-loop dialogs for the bridge `extension_ui_request`
//! methods `select` / `confirm` / `input` / `editor`. The core forwards the
//! request to the TUI (headless keeps the default-answers path); the user
//! answers in the terminal; the answer flows back over the unchanged bridge
//! protocol as `extension_ui_response`.
//!
//! Timeouts: the sidecar's own timer is authoritative (it resolves the
//! dialog with the upstream default). The TUI only stops showing the dialog
//! at the same deadline and sends nothing on expiry.
use std::time::{Duration, Instant};

use crossterm::event::{Event as TermEvent, KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum Dialog {
    Select {
        id: String,
        title: String,
        options: Vec<String>,
        cursor: usize,
        deadline: Instant,
    },
    Confirm {
        id: String,
        title: String,
        message: String,
        deadline: Instant,
    },
    Input {
        id: String,
        title: String,
        placeholder: String,
        text: String,
        deadline: Instant,
    },
}

/// What the terminal loop reports for a closed dialog: the bridge response
/// (answered) or a no-response timeout close (the sidecar already resolved
/// with its default).
#[derive(Debug, Clone)]
pub enum DialogOutcome {
    Answered {
        id: String,
        method: String,
        title: String,
        message: String,
        response: Value,
    },
    TimedOut {
        id: String,
        method: String,
        title: String,
        message: String,
    },
}

impl Dialog {
    /// Build a dialog from a bridge `extension_ui_request` (dialog subset
    /// only; `None` for F&F methods and unknown shapes).
    pub fn from_request(ev: &Value) -> Option<Self> {
        let id = ev.get("id")?.as_str()?.to_string();
        let title = ev.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
        let timeout_ms = ev.get("timeout").and_then(Value::as_u64).unwrap_or(30_000);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        match ev.get("method")?.as_str()? {
            "select" => {
                let options = ev
                    .get("options")?
                    .as_array()?
                    .iter()
                    .map(|o| o.as_str().unwrap_or_default().to_string())
                    .collect::<Vec<_>>();
                Some(Dialog::Select { id, title, options, cursor: 0, deadline })
            }
            "confirm" => {
                let message = ev.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
                Some(Dialog::Confirm { id, title, message, deadline })
            }
            "input" => {
                let placeholder = ev.get("placeholder").and_then(Value::as_str).unwrap_or_default().to_string();
                Some(Dialog::Input { id, title, placeholder, text: String::new(), deadline })
            }
            "editor" => {
                let prefill = ev.get("prefill").and_then(Value::as_str).unwrap_or_default().to_string();
                Some(Dialog::Input { id, title, placeholder: String::new(), text: prefill, deadline })
            }
            _ => None, // notify / setStatus / setTitle / set_editor_text: F&F
        }
    }

    fn method(&self) -> &'static str {
        match self {
            Dialog::Select { .. } => "select",
            Dialog::Confirm { .. } => "confirm",
            Dialog::Input { .. } => "input",
        }
    }

    fn meta(&self) -> (String, String, String) {
        // (id, title, message) for the log line.
        match self {
            Dialog::Select { id, title, .. } => (id.clone(), title.clone(), String::new()),
            Dialog::Confirm { id, title, message, .. } => (id.clone(), title.clone(), message.clone()),
            Dialog::Input { id, title, .. } => (id.clone(), title.clone(), String::new()),
        }
    }

    pub fn id(&self) -> String {
        self.meta().0
    }

    pub fn expired(&self, now: Instant) -> bool {
        match self {
            Dialog::Select { deadline, .. }
            | Dialog::Confirm { deadline, .. }
            | Dialog::Input { deadline, .. } => *deadline <= now,
        }
    }

    pub fn timed_out(&self) -> DialogOutcome {
        let (id, title, message) = self.meta();
        DialogOutcome::TimedOut { id, method: self.method().to_string(), title, message }
    }

    fn answered(&self, response: Value) -> DialogOutcome {
        let (id, title, message) = self.meta();
        DialogOutcome::Answered { id, method: self.method().to_string(), title, message, response }
    }

    fn cancel(id: &str) -> Value {
        json!({"type":"extension_ui_response","id":id,"cancelled":true})
    }

    /// Handle one key while the dialog is open. Returns the outcome when the
    /// dialog closes; `None` while it stays open (the key was consumed).
    pub fn handle_key(&mut self, ev: &TermEvent) -> Option<DialogOutcome> {
        #[derive(Clone, Copy, PartialEq)]
        enum Act {
            Pending,
            Pick,
            Yes,
            No,
            Submit,
            Cancel,
        }
        let is_esc = |ev: &TermEvent| {
            matches!(ev, TermEvent::Key(k) if k.code == KeyCode::Esc)
                || matches!(ev, TermEvent::Key(k) if k.modifiers.contains(KeyModifiers::CONTROL)
                    && k.code == KeyCode::Char('c'))
        };
        let act = match self {
            Dialog::Select { options, cursor, .. } => match ev {
                TermEvent::Key(k) if k.code == KeyCode::Up => {
                    if !options.is_empty() {
                        *cursor = options.len().wrapping_sub(*cursor + 1) % options.len();
                    }
                    Act::Pending
                }
                TermEvent::Key(k) if k.code == KeyCode::Down => {
                    if !options.is_empty() {
                        *cursor = (*cursor + 1) % options.len();
                    }
                    Act::Pending
                }
                TermEvent::Key(k) if k.code == KeyCode::Enter => Act::Pick,
                _ if is_esc(ev) => Act::Cancel,
                _ => Act::Pending,
            },
            Dialog::Confirm { .. } => match ev {
                TermEvent::Key(k)
                    if (k.code == KeyCode::Enter
                        || k.code == KeyCode::Char('y')
                        || k.code == KeyCode::Char('Y'))
                        && !k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    Act::Yes
                }
                TermEvent::Key(k)
                    if (k.code == KeyCode::Char('n') || k.code == KeyCode::Char('N'))
                        && !k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    Act::No
                }
                _ if is_esc(ev) => Act::Cancel,
                _ => Act::Pending,
            },
            Dialog::Input { text, .. } => match ev {
                TermEvent::Key(k) if k.code == KeyCode::Enter => Act::Submit,
                TermEvent::Key(k) if k.code == KeyCode::Backspace => {
                    text.pop();
                    Act::Pending
                }
                TermEvent::Key(k)
                    if matches!(k.code, KeyCode::Char(_)) && !k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if let KeyCode::Char(c) = k.code {
                        text.push(c);
                    }
                    Act::Pending
                }
                _ if is_esc(ev) => Act::Cancel,
                _ => Act::Pending,
            },
        };
        if act == Act::Pending {
            return None;
        }
        let response = match (act, &*self) {
            (Act::Pick, Dialog::Select { id, options, cursor, .. }) => json!({"type":"extension_ui_response","id":id,"value":options.get(*cursor).cloned()}),
            (yes_no @ (Act::Yes | Act::No), Dialog::Confirm { id, .. }) => json!({"type":"extension_ui_response","id":id,"confirmed":yes_no == Act::Yes}),
            (Act::Submit, Dialog::Input { id, text, .. }) => json!({"type":"extension_ui_response","id":id,"value":text}),
            (_, d) => Self::cancel(&d.id()),
        };
        Some(self.answered(response))
    }

    /// Render the dialog as transcript-area lines (the pane shows these on
    /// top while the dialog is open).
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        let title_style = Style::default().add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(Color::DarkGray);
        let accent = Style::default().fg(Color::Cyan);
        match self {
            Dialog::Select { title, options, cursor, .. } => {
                let mut lines = vec![Line::from(Span::styled(title.clone(), title_style))];
                for (i, opt) in options.iter().enumerate() {
                    let marker = if i == *cursor { "▸ " } else { "  " };
                    let style = if i == *cursor { accent } else { Style::default() };
                    lines.push(Line::from(vec![
                        Span::styled(marker.to_string(), style.clone()),
                        Span::styled(opt.clone(), style),
                    ]));
                }
                lines.push(Line::from(Span::styled(
                    "↑↓ choose · enter ok · esc cancel",
                    dim,
                )));
                lines
            }
            Dialog::Confirm { title, message, .. } => {
                let mut lines = vec![Line::from(Span::styled(title.clone(), title_style))];
                for m in message.lines() {
                    lines.push(Line::from(Span::styled(m.to_string(), Style::default())));
                }
                lines.push(Line::from(Span::styled(
                    "y yes · n no · enter yes · esc cancel",
                    dim,
                )));
                lines
            }
            Dialog::Input { title, placeholder, text, .. } => {
                let mut lines = vec![Line::from(Span::styled(title.clone(), title_style))];
                if text.is_empty() && !placeholder.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("> {placeholder}"),
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("> ".to_string(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(text.clone()),
                    ]));
                }
                lines.push(Line::from(Span::styled("enter ok · esc cancel", dim)));
                lines
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(c: char) -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn key_code(code: KeyCode) -> TermEvent {
        TermEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn esc() -> TermEvent {
        TermEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    }

    fn select_req() -> Value {
        json!({"id":"d1","method":"select","title":"Pick","options":["alpha","beta"],"timeout":60000})
    }

    fn answered_value(outcome: &Option<DialogOutcome>) -> Option<Value> {
        match outcome {
            Some(DialogOutcome::Answered { response, .. }) => Some(response.clone()),
            _ => None,
        }
    }

    #[test]
    fn select_from_request_and_answer() {
        let mut d = Dialog::from_request(&select_req()).unwrap();
        // first option is preselected
        let r = answered_value(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["value"], json!("alpha"), "{r}");
        // down moves the cursor, enter picks it
        let mut d = Dialog::from_request(&select_req()).unwrap();
        assert!(d.handle_key(&key_code(KeyCode::Down)).is_none());
        let r = answered_value(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["value"], json!("beta"), "{r}");
        // up wraps from 0 to last
        let mut d = Dialog::from_request(&select_req()).unwrap();
        assert!(d.handle_key(&key_code(KeyCode::Up)).is_none());
        let r = answered_value(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["value"], json!("beta"), "{r}");
        // esc cancels
        let mut d = Dialog::from_request(&select_req()).unwrap();
        let r = answered_value(&d.handle_key(&esc())).unwrap();
        assert_eq!(r["cancelled"], json!(true), "{r}");
    }

    fn answered_bool(outcome: &Option<DialogOutcome>) -> Option<Value> {
        match outcome {
            Some(DialogOutcome::Answered { response, .. }) => Some(response.clone()),
            _ => None,
        }
    }

    #[test]
    fn confirm_y_n_enter_esc() {
        let req = json!({"id":"d2","method":"confirm","title":"Proceed?","message":"3 files","timeout":60000});
        let mut d = Dialog::from_request(&req).unwrap();
        let r = answered_bool(&d.handle_key(&key('y'))).unwrap();
        assert_eq!(r["confirmed"], json!(true), "{r}");
        let mut d = Dialog::from_request(&req).unwrap();
        let r = answered_bool(&d.handle_key(&key('n'))).unwrap();
        assert_eq!(r["confirmed"], json!(false), "{r}");
        let mut d = Dialog::from_request(&req).unwrap();
        let r = answered_bool(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["confirmed"], json!(true), "{r}");
        let mut d = Dialog::from_request(&req).unwrap();
        let r = answered_bool(&d.handle_key(&esc())).unwrap();
        assert_eq!(r["cancelled"], json!(true), "{r}");
    }

    #[test]
    fn input_types_and_submits() {
        let req = json!({"id":"d3","method":"input","title":"Name","placeholder":"type here","timeout":60000});
        let mut d = Dialog::from_request(&req).unwrap();
        for c in "amy".chars() {
            assert!(d.handle_key(&key(c)).is_none());
        }
        let r = answered_value(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["value"], json!("amy"), "{r}");
        // backspace + esc cancel
        let mut d = Dialog::from_request(&req).unwrap();
        d.handle_key(&key('a'));
        d.handle_key(&key('b'));
        assert!(d.handle_key(&key_code(KeyCode::Backspace)).is_none());
        let r = answered_value(&d.handle_key(&esc())).unwrap();
        assert_eq!(r["cancelled"], json!(true), "{r}");
        // editor prefill
        let mut d = Dialog::from_request(&json!({"id":"d4","method":"editor","title":"Note","prefill":"hi","timeout":60000})).unwrap();
        let r = answered_value(&d.handle_key(&key_code(KeyCode::Enter))).unwrap();
        assert_eq!(r["value"], json!("hi"), "{r}");
    }

    #[test]
    fn ff_methods_are_not_dialogs() {
        assert!(Dialog::from_request(&json!({"id":"x","method":"notify","message":"hi"})).is_none());
        assert!(Dialog::from_request(&json!({"id":"x","method":"setStatus","statusKey":"k","statusText":"t"})).is_none());
        assert!(Dialog::from_request(&json!({"id":"x","method":"setTitle","title":"t"})).is_none());
    }

    #[test]
    fn expiry_yields_timeout_outcome() {
        let req = json!({"id":"d5","method":"confirm","title":"t","message":"m","timeout":1});
        let mut d = Dialog::from_request(&req).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(d.expired(Instant::now()));
        match d.timed_out() {
            DialogOutcome::TimedOut { method, .. } => assert_eq!(method, "confirm"),
            other => panic!("expected TimedOut, got {other:?}"),
        }
    }

    #[test]
    fn render_lines_show_title_and_selection() {
        let d = Dialog::from_request(&select_req()).unwrap();
        let lines = d.render_lines();
        let joined: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect();
        assert!(joined.first() == Some(&"Pick".to_string()), "{joined:?}");
        assert!(joined[1].starts_with('▸'), "first option preselected: {joined:?}");
        assert!(joined[2].starts_with(' '), "second option not selected: {joined:?}");
        assert!(joined.last().unwrap().contains("esc"), "{joined:?}");
    }

}
