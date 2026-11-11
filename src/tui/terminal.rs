//! WP-601: thin crossterm adapter around `TuiApp`. Raw mode + alt screen,
//! a blocking stdin reader thread (crossterm's blocking API) feeding the
//! async TUI loop, periodic redraws for the pending indicator.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::Value;
use tokio::sync::mpsc;

/// Run the TUI loop until the app sets `exit`.
/// - `events`: core event broadcast (EventLog stream).
/// - `on_submit`: called with the submitted input text (async).
/// - `on_exit`: called once when the user quits (async).
pub async fn run<F, G>(
    app: &mut super::TuiApp,
    mut events: tokio::sync::broadcast::Receiver<Value>,
    mut on_submit: F,
    mut on_exit: G,
) -> anyhow::Result<()>
where
    F: FnMut(&str) + Send,
    G: FnMut() + Send,
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
    execute!(backend, EnterAlternateScreen)?;
    let mut term = Terminal::new(backend).context("init terminal")?;
    app.set_size(term.size().map(|s| (s.width, s.height)).unwrap_or((80, 24)).0,
                 term.size().map(|s| (s.width, s.height)).unwrap_or((80, 24)).1);

    // Redraws are event-driven (keys, core events, resizes): the pending
    // indicator changes only on agent_start/agent_end, so no idle tick is
    // needed (and idle redraws would spam cursor-hide escapes every 50ms).
    let mut dirty = true;

    let result = (async {
        loop {
            // The pty/terminal size can change at any time (startup race,
            // user resize): re-read it before every draw.
            if let Err(e) = term.autoresize() {
                eprintln!("[tui] resize: {e}");
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
                        if let Some(text) = app.handle_key(e) {
                            on_submit(&text);
                        }
                        dirty = true;
                        if app.exit {
                            on_exit();
                            break;
                        }
                    }
                    None => break,
                },
                ev = events.recv() => match ev {
                    Ok(v) => {
                        app.apply_event(&v);
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
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    let _ = terminal::disable_raw_mode();
    let _ = reader.join();
    result
}
