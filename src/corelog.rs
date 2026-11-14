//! Core diagnostic lines (`[core] ...`). Headless mode prints them to stderr
//! as before; TUI mode owns the screen, so they go to a log file instead —
//! stderr lines interleaved with ratatui's diff output would corrupt the
//! rendered frame (the sidecar's stderr is redirected the same way).
use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

static SINK: Mutex<Option<File>> = Mutex::new(None);

/// Route subsequent lines to `path` (created/appended) instead of stderr.
/// Keeps stderr if the file cannot be opened.
pub fn set_log_file(path: impl AsRef<std::path::Path>) {
    if let Ok(f) = File::options().create(true).append(true).open(path) {
        *SINK.lock().unwrap() = Some(f)
    }
}

pub fn line(msg: &str) {
    let mut g = SINK.lock().unwrap();
    match g.as_mut() {
        Some(f) => {
            let _ = writeln!(f, "{msg}");
        }
        None => eprintln!("{msg}"),
    }
}
