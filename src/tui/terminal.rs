//! WP-601: thin crossterm adapter around `TuiApp`. Raw mode + alt screen,
//! a blocking stdin reader thread (crossterm's blocking API) feeding the
//! async TUI loop, event-driven redraws.
//!
//! WP-604 additions: the `ui` channel carries dialog requests
//! (`extension_ui_request` select/confirm/input/editor) and `on_dialog`
//! reports each closed dialog (answered -> the caller sends the bridge
//! response; timed out -> no response, the sidecar resolved with its
//! upstream default).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::EnableBracketedPaste;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// Run the TUI loop until the app sets `exit`.
/// - `events`: core event broadcast (EventLog stream).
/// - `ui`: dialog requests from the core (WP-604).
/// - `on_submit`: called with the submitted input text.
/// - `on_dialog`: called when a dialog closes (WP-604).
/// - `agent_dir`: used by `/theme <name>` to load the named theme (WP-607);
///   an invalid theme is shown in the status bar, the old theme stays.
/// - `on_request`: WP-608 slash commands that need the bridge; the closure
///   sends the request and renders the result via a TUI-only event.
/// - `on_resume`: WP-609 session resume — loads the file and swaps the
///   core's active session; the return carries the loaded session for the
///   TUI display reset.
/// - `on_exit`: called once when the user quits.
pub async fn run<F, G, H, I, J>(
    app: &mut super::TuiApp,
    mut events: tokio::sync::broadcast::Receiver<Value>,
    mut ui: mpsc::UnboundedReceiver<Value>,
    mut on_submit: F,
    mut on_dialog: H,
    agent_dir: std::path::PathBuf,
    mut on_request: I,
    mut on_resume: J,
    mut on_exit: G,
) -> anyhow::Result<()>
where
    F: FnMut(&str) + Send,
    G: FnMut() + Send,
    H: FnMut(super::dialog::DialogOutcome) + Send,
    I: FnMut(String, Value, &str, Value) + Send,
    J: FnMut(&str) -> Result<(String, Vec<Value>), String> + Send,
{
    // Stdin reader thread: crossterm's blocking poll+read into a channel.
    // The stop flag bounds the final join: without it the reader only
    // notices the dropped channel on the *next* key event (never comes on
    // quit), so the join would hang the whole shutdown.
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<TermEvent>();
    let reader_stop = Arc::new(AtomicBool::new(false));
    let reader_stop_r = reader_stop.clone();
    let reader = std::thread::spawn(move || loop {
        if reader_stop_r.load(Ordering::Relaxed) {
            break;
        }
        match event::poll(Duration::from_millis(50)) {
            Ok(true) => {
                match event::read() {
                    Ok(ev) => {
                        if stdin_tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => {}
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    });

    let mut backend = CrosstermBackend::new(std::io::stdout());
    terminal::enable_raw_mode().context("enabling raw mode")?;
    execute!(backend, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut term = Terminal::new(backend).context("init terminal")?;
    app.set_size(term.size().map(|s| (s.width, s.height)).unwrap_or((80, 24)).0,
                 term.size().map(|s| (s.width, s.height)).unwrap_or((80, 24)).1);

    // Redraws are event-driven (keys, core events, resizes, dialogs).
    let mut dirty = true;

    let result = (async {
        loop {
            // The pty/terminal size can change at any time (startup race,
            // user resize): re-read it before every draw.
            if let Err(e) = term.autoresize() {
                eprintln!("[tui] resize: {e}");
            }
            // WP-604: a dialog whose sidecar-side timeout fired stops being
            // shown; the sidecar already resolved with its default, so no
            // bridge response goes out.
            if let Some(outcome) = app.take_expired_dialog(std::time::Instant::now()) {
                on_dialog(outcome);
                dirty = true;
            }
            if dirty {
                term.draw(|f| {
                    let area = f.area();
                    app.render_to(f.buffer_mut(), area);
                })?;
                dirty = false;
            }
            tokio::select! {
                ev = stdin_rx.recv() => match ev {
                    Some(e) => {
                        // WP-609: an open picker owns the keys.
                        if app.picker.is_some() {
                            if let Some(outcome) = app.handle_picker_key(&e) {
                                let kind = app.picker_kind;
                                app.picker = None;
                                match (kind, outcome) {
                                    (super::PickerKind::Model, super::picker::PickerOutcome::Selected { value, .. }) => {
                                        if let Some((provider, model)) = value.split_once('/') {
                                            on_request(
                                                format!("/model {value}"),
                                                json!({ "model": value }),
                                                "set_model_request",
                                                json!({ "provider": provider, "modelId": model }),
                                            );
                                        }
                                    }
                                    (super::PickerKind::Session, super::picker::PickerOutcome::Selected { value, .. }) => {
                                        match on_resume(&value) {
                                            Ok((id, msgs)) => {
                                                app.session = id;
                                                let mut transcript = Vec::new();
                                                for m in msgs {
                                                    let text = crate::session::extract_text(&m);
                                                    let role: &'static str =
                                                        match m.get("role").and_then(Value::as_str) {
                                                            Some("assistant") => "assistant",
                                                            _ => "user",
                                                        };
                                                    transcript.push(super::Msg { role, text });
                                                }
                                                app.transcript = transcript;
                                                app.scroll = 0;
                                            }
                                            Err(e) => {
                                                crate::corelog::line(&format!("[core] resume: {e}"));
                                                app.last_error = Some(format!("resume: {e}"));
                                            }
                                        }
                                    }
                                    (_, super::picker::PickerOutcome::Cancelled) => {}
                                }
                            }
                        } else if let Some(outcome) = app.handle_dialog_key(&e) {
                            on_dialog(outcome);
                        } else if let Some(action) = app.handle_key(e) {
                            match action {
                                super::TuiAction::Submit(text) => on_submit(&text),
                                super::TuiAction::EditorClosed(outcome) => on_dialog(outcome),
                                super::TuiAction::SwitchTheme(name) => {
                                    // WP-607: in-place theme switch; an invalid
                                    // theme keeps the old one and reports.
                                    match super::theme::Theme::load(&agent_dir, &name) {
                                        Ok(t) => {
                                            app.theme = t;
                                            app.last_error = None;
                                        }
                                        Err(e) => {
                                            crate::corelog::line(&format!("[core] theme: {e}"));
                                            app.last_error = Some(format!("theme: {e}"));
                                        }
                                    }
                                }
                                super::TuiAction::BridgeCommand { name, args } => {
                                    on_request(format!("/{name}"), Value::Null, "command", json!({ "name": name, "args": args }));
                                }
                                super::TuiAction::SetThinking { level } => {
                                    on_request(format!("/thinking {level}"), Value::Null, "set_thinking_level_request", json!({ "level": level }));
                                }
                                super::TuiAction::SetModel { provider, model } => {
                                    on_request(
                                        format!("/model {provider}/{model}"),
                                        json!({ "model": format!("{provider}/{model}") }),
                                        "set_model_request",
                                        json!({ "provider": provider, "modelId": model }),
                                    );
                                }
                                super::TuiAction::OpenSessionsPicker => {
                                    // WP-609: the session list (shared with
                                    // --list-sessions) feeds the picker.
                                    let infos = crate::session::list(&agent_dir);
                                    if infos.is_empty() {
                                        app.last_error =
                                            Some("no sessions to resume under the agent dir".into());
                                    } else {
                                        let items = infos
                                            .iter()
                                            .map(|s| {
                                                let label = s
                                                    .name
                                                    .clone()
                                                    .unwrap_or_else(|| format!("session {}", &s.id[..s.id.len().min(8)]));
                                                let display = format!("{label} \u{b7} {}", s.updated.as_deref().unwrap_or("-"));
                                                super::picker::PickerItem { display, value: s.path.to_string_lossy().to_string() }
                                            })
                                            .collect();
                                        app.picker_kind = super::PickerKind::Session;
                                        app.picker = Some(super::picker::Picker::new("Resume session", items));
                                    }
                                }
                            }
                        }
                        dirty = true;
                        if app.exit {
                            on_exit();
                            break;
                        }
                    }
                    None => break,
                },
                // WP-604: dialog request from the core.
                req = ui.recv() => match req {
                    Some(v) => {
                        // WP-605: the `editor` dialog reuses the editor pane.
                        let opened = if v.get("method").and_then(serde_json::Value::as_str) == Some("editor") {
                            app.open_editor_session(&v)
                        } else {
                            app.open_dialog(&v)
                        };
                        if opened {
                            dirty = true;
                        }
                    }
                    None => break,
                },
                ev = events.recv() => match ev {
                    // WP-603: drain the burst before drawing — a stream of
                    // token deltas becomes one render, not one per token.
                    Ok(first) => {
                        let mut v = first;
                        loop {
                            app.apply_event(&v);
                            // WP-607: theme switch from the core (pi.setTheme).
                            if v.get("kind").and_then(|k| k.as_str()) == Some("set_theme") {
                                if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                                    match super::theme::Theme::load(&agent_dir, name) {
                                        Ok(t) => {
                                            app.theme = t;
                                            app.last_error = None;
                                        }
                                        Err(e) => {
                                            crate::corelog::line(&format!("[core] theme: {e}"));
                                            app.last_error = Some(format!("theme: {e}"));
                                        }
                                    }
                                }
                            }
                            match events.try_recv() {
                                Ok(more) => v = more,
                                Err(_) => break,
                            }
                        }
                        dirty = true;
                    }
                    // Receiver lagged: drop and continue (events are display-only).
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => { dirty = true; }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        Ok(())
    })
    .await;

    // Always restore the terminal, even on error. Stop the reader first so
    // the join returns within one poll interval instead of hanging.
    reader_stop.store(true, Ordering::Relaxed);
    let _ = term.show_cursor();
    execute!(
        term.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    let _ = terminal::disable_raw_mode();
    let _ = reader.join();
    result
}
