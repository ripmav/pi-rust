use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

use crate::edit_diff;

/// v0 built-in tools: `bash`, `read`, `write`, `edit`.
pub fn schemas() -> Vec<Value> {
    vec![
        json!({"name":"bash","description":"Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.","parameters":{"type":"object","properties":{"command":{"type":"string","description":"Shell command to execute"},"timeout":{"type":"number","description":"Timeout in seconds (optional, no default timeout)"}},"required":["command"]}}),
        json!({"name":"read","description":"Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to read (relative or absolute)"},"offset":{"type":"number","description":"Line number to start reading from (1-indexed)"},"limit":{"type":"number","description":"Maximum number of lines to read"}},"required":["path"]}}),
        json!({"name":"write","description":"Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to write (relative or absolute)"},"content":{"type":"string","description":"Content to write to the file"}},"required":["path","content"]}}),
        json!({"name":"edit","description":"Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Path to the file to edit (relative or absolute)"},"edits":{"type":"array","description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.","items":{"type":"object","properties":{"oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},"newText":{"type":"string","description":"Replacement text for this targeted edit."}},"required":["oldText","newText"]}}},"required":["path","edits"]}}),
    ]
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "bash" | "read" | "write" | "edit")
}

/// Upstream registry precedence (agent-session _refreshToolRegistry): extension
/// tools override built-ins with the same name (shadowing) — a shadowed
/// built-in keeps its position, new extension tools append after it.
pub fn merge_with_extensions(builtins: Vec<Value>, ext: Vec<Value>) -> Vec<Value> {
    let mut defs = builtins;
    for e in ext {
        let Some(name) = e.get("name").and_then(Value::as_str) else {
            defs.push(e);
            continue;
        };
        match defs.iter().position(|d| d.get("name").and_then(Value::as_str) == Some(name)) {
            Some(i) => defs[i] = e,
            None => defs.push(e),
        }
    }
    defs
}

pub struct ExecResult {
    pub content: Value,
    pub details: Value,
    pub is_error: bool,
    /// Terminate hint from an extension `AgentToolResult` (built-ins never set it).
    pub terminate: bool,
}

impl ExecResult {
    fn text(t: String) -> Self {
        Self { content: json!([{"type":"text","text":t}]), details: json!({}), is_error: false, terminate: false }
    }
    fn err(t: String) -> Self {
        let mut r = Self::text(t);
        r.is_error = true;
        r
    }
}

pub async fn execute(name: &str, input: &Value) -> ExecResult {
    match name {
        "bash" => bash(input).await,
        "read" => read(input),
        "write" => write(input),
        "edit" => edit(input),
        _ => ExecResult::err(format!("unknown built-in tool: {name}")),
    }
}

/// Upstream `bash` limits (truncate.ts): keep the LAST 2000 lines / 50KB (whichever is hit
/// first) — bash output is tail-truncated, unlike `read` which is head-truncated.
const BASH_MAX_LINES: usize = 2000;
const BASH_MAX_BYTES: usize = 50 * 1024;

/// The tail truncation result in the exact upstream `TruncationResult` shape (tail variant).
struct TailTruncation {
	content: String,
	truncated_by: &'static str,
	total_lines: usize,
	total_bytes: u64,
	output_lines: usize,
	output_bytes: usize,
	last_line_partial: bool,
	last_line_bytes: u64,
	full_output_path: PathBuf,
}

impl TailTruncation {
	fn to_json(&self) -> Value {
		json!({
			"content": self.content,
			"truncated": true,
			"truncatedBy": self.truncated_by,
			"totalLines": self.total_lines,
			"totalBytes": self.total_bytes,
			"outputLines": self.output_lines,
			"outputBytes": self.output_bytes,
			"lastLinePartial": self.last_line_partial,
			"firstLineExceedsLimit": false,
			"maxLines": BASH_MAX_LINES,
			"maxBytes": BASH_MAX_BYTES,
		})
	}
}

/// Rolling capture of the combined stdout/stderr stream. Mirrors every chunk to a
/// `pi-bash-*.log` temp file and keeps only the last 50KB+1 bytes in memory: the display
/// can only ever reference the last 50KB of the content (the +1 keeps the last 50KB of an
/// oversized final line even when the content ends with a newline), so memory stays
/// bounded no matter how much the command prints.
struct BashCapture {
	tail: Vec<u8>,
	/// The window cut landed mid-line, so the tail's first line is a fragment whose true
	/// size cannot fit in any remainder (window arithmetic), and the upstream backward
	/// loop breaks on it.
	front_partial: bool,
	total_bytes: u64,
	newlines: u64,
	last_line_bytes: u64,
	ends_with_nl: bool,
	path: PathBuf,
	file: Option<std::fs::File>,
}

impl BashCapture {
	fn new() -> Self {
		let path = std::env::temp_dir().join(format!("pi-bash-{}.log", uuid::Uuid::new_v4()));
		let file = std::fs::File::create(&path).ok();
		Self { tail: Vec::new(), front_partial: false, total_bytes: 0, newlines: 0, last_line_bytes: 0, ends_with_nl: false, path, file }
	}

	fn feed(&mut self, chunk: &[u8]) {
		if chunk.is_empty() {
			return;
		}
		self.total_bytes += chunk.len() as u64;
		if let Some(last) = chunk.iter().rposition(|b| *b == b'\n') {
			self.last_line_bytes = (chunk.len() - last - 1) as u64;
		} else {
			self.last_line_bytes += chunk.len() as u64;
		}
		self.newlines += chunk.iter().filter(|b| **b == b'\n').count() as u64;
		self.ends_with_nl = chunk.last() == Some(&b'\n');
		// shortcut: blocking write per chunk keeps the pump simple; bash output is usually
		// small and the 10MB test streams 8KB chunks.
		if let Some(f) = &mut self.file {
			let _ = f.write_all(chunk);
		}
		self.tail.extend(chunk);
		let limit = BASH_MAX_BYTES + 1;
		if self.tail.len() > limit {
			let cut = self.tail.len() - limit;
			self.front_partial = self.tail[cut - 1] != b'\n';
			self.tail.drain(0..cut);
		}
	}

	/// Finalize: drop the temp file when nothing was truncated, otherwise build the
	/// tail truncation result (upstream `truncateTail` over the rolling window).
	fn finish(mut self) -> (String, Option<TailTruncation>) {
		let total_lines = if self.total_bytes == 0 {
			0
		} else {
			(self.newlines + u64::from(!self.ends_with_nl)) as usize
		};
		let raw = std::mem::take(&mut self.tail);
		let content = String::from_utf8_lossy(&raw).into_owned();
		if total_lines <= BASH_MAX_LINES && self.total_bytes <= BASH_MAX_BYTES as u64 {
			drop(self.file);
			let _ = std::fs::remove_file(&self.path);
			return (content, None);
		}
		let mut t = truncate_tail_capture(&raw, self.front_partial, total_lines, self.total_bytes, self.last_line_bytes);
		t.full_output_path = self.path.clone();
		(t.content.clone(), Some(t))
	}
}

/// Upstream `truncateTail` mirrored: work backwards from the end, add complete lines while
/// they fit in 50KB, stop at 2000 lines; an oversized final line takes its last 50KB as a
/// partial first line. Runs over the rolling window (last 50KB+1 bytes), which provably
/// contains every line the display can reference.
fn truncate_tail_capture(raw: &[u8], front_partial: bool, total_lines: usize, total_bytes: u64, last_line_bytes: u64) -> TailTruncation {
	let max_bytes = BASH_MAX_BYTES;
	let mut truncated_by = "lines";
	let mut last_line_partial = false;
	let (out, output_lines, output_bytes) = if last_line_bytes as usize > max_bytes {
		// Edge case: the final line alone exceeds the byte limit — show the end of the line.
		let line = if raw.last() == Some(&b'\n') { &raw[..raw.len() - 1] } else { raw };
		let mut start = line.len().saturating_sub(max_bytes);
		// Land on a UTF-8 character boundary (upstream truncateStringToBytesFromEnd).
		while start < line.len() && (line[start] & 0xc0) == 0x80 {
			start += 1;
		}
		let s = String::from_utf8_lossy(&line[start..]).into_owned();
		let len = s.len();
		truncated_by = "bytes";
		last_line_partial = true;
		(s, 1, len)
	} else {
		let text = String::from_utf8_lossy(raw);
		let mut lines: Vec<&str> = text.split('\n').collect();
		if text.ends_with('\n') {
			lines.pop();
		}
		let mut arr: Vec<&str> = Vec::new();
		let mut bytes = 0usize;
		for idx in (0..lines.len()).rev() {
			if arr.len() >= BASH_MAX_LINES {
				break;
			}
			if idx == 0 && front_partial {
				truncated_by = "bytes";
				break;
			}
			let line = lines[idx];
			let lb = line.len() + if arr.is_empty() { 0 } else { 1 };
			if bytes + lb > max_bytes {
				truncated_by = "bytes";
				break;
			}
			arr.insert(0, line);
			bytes += lb;
		}
		if arr.len() >= BASH_MAX_LINES && bytes <= max_bytes {
			truncated_by = "lines";
		}
		let joined = arr.join("\n");
		let len = joined.len();
		(joined, arr.len(), len)
	};
	TailTruncation { content: out, truncated_by, total_lines, total_bytes, output_lines, output_bytes, last_line_partial, last_line_bytes, full_output_path: PathBuf::new() }
}

/// Read both pipes until EOF, feeding chunks to the capture in arrival order (upstream
/// merges both streams into one accumulator — no `[stderr]` block). `biased` mirrors
/// libuv's handle order: when both pipes are ready at once, stdout is drained first.
async fn pump_child(child: &mut Child, capture: &mut BashCapture) -> Option<ExitStatus> {
	let mut stdout = child.stdout.take();
	let mut stderr = child.stderr.take();
	let mut sbuf = [0u8; 8192];
	let mut ebuf = [0u8; 8192];
	let mut s_done = stdout.is_none();
	let mut e_done = stderr.is_none();
	while !s_done || !e_done {
		tokio::select! {
			biased;
			r = stdout.as_mut().unwrap().read(&mut sbuf), if !s_done => match r {
				Ok(0) | Err(_) => s_done = true,
				Ok(n) => capture.feed(&sbuf[..n]),
			},
			r = stderr.as_mut().unwrap().read(&mut ebuf), if !e_done => match r {
				Ok(0) | Err(_) => e_done = true,
				Ok(n) => capture.feed(&ebuf[..n]),
			},
		}
	}
	child.wait().await.ok()
}

async fn bash(input: &Value) -> ExecResult {
	let Some(command) = input.get("command").and_then(Value::as_str) else {
		return ExecResult::err("bash: missing string field `command`".into());
	};
	// Upstream `resolveTimeoutMs`: finite positive seconds, max 2^31-1 ms
	// (JS prints the maximum as 2147483.647).
	let timeout = match input.get("timeout") {
		None => None,
		Some(v) => {
			let t = match v.as_f64() {
				Some(t) if t.is_finite() && t > 0.0 => t,
				_ => return ExecResult::err("Invalid timeout: must be a finite number of seconds".into()),
			};
			if t * 1000.0 > 2_147_483_647.0 {
				return ExecResult::err("Invalid timeout: maximum is 2147483.647 seconds".into());
			}
			Some((t, std::time::Duration::from_secs_f64(t)))
		}
	};
	let Ok(mut child) = Command::new("bash").arg("-c").arg(command).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else {
		return ExecResult::err("bash: failed to start".into());
	};
	let mut capture = BashCapture::new();
	let (status, timed_out) = match timeout {
		None => (pump_child(&mut child, &mut capture).await, false),
		Some((_, dur)) => match tokio::time::timeout(dur, pump_child(&mut child, &mut capture)).await {
			Ok(s) => (s, false),
			Err(_) => {
				let _ = child.start_kill();
				let _ = child.wait().await;
				(None, true)
			}
		}
	};
	let (content, trunc) = capture.finish();
	// Upstream: the "(no output)" placeholder is only applied on non-error paths
	// (formatOutput's default emptyText); error paths pass "".
	let mut text = if content.is_empty() && !timed_out { "(no output)".to_string() } else { content };
	let mut details = json!({});
	if let Some(t) = &trunc {
		let start_line = t.total_lines - t.output_lines + 1;
		let end_line = t.total_lines;
		if t.last_line_partial {
			text.push_str(&format!("\n\n[Showing last {} of line {end_line} (line is {}). Full output: {}]", format_size(t.output_bytes), format_size(t.last_line_bytes as usize), t.full_output_path.display()));
		} else if t.truncated_by == "lines" {
			text.push_str(&format!("\n\n[Showing lines {start_line}-{end_line} of {end_line}. Full output: {}]", t.full_output_path.display()));
		} else {
			text.push_str(&format!("\n\n[Showing lines {start_line}-{end_line} of {end_line} ({} limit). Full output: {}]", format_size(BASH_MAX_BYTES), t.full_output_path.display()));
		}
		details = json!({"truncation": t.to_json(), "fullOutputPath": t.full_output_path.to_string_lossy().into_owned()});
	}
	if timed_out {
		let secs = timeout.map(|(t, _)| t.to_string()).unwrap_or_default();
		let status = format!("Command timed out after {secs} seconds");
		let full = if text.is_empty() { status } else { format!("{text}\n\n{status}") };
		return ExecResult { content: json!([{"type":"text","text":full}]), details, is_error: true, terminate: false };
	}
	let code = match status {
		Some(s) if s.success() => 0,
		Some(s) => s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0) as i32),
		None => 1,
	};
	if code == 0 {
		ExecResult { content: json!([{"type":"text","text":text}]), details, is_error: false, terminate: false }
	} else {
		let full = format!("{text}\n\nCommand exited with code {code}");
		ExecResult { content: json!([{"type":"text","text":full}]), details, is_error: true, terminate: false }
	}
}

/// Upstream read-tool caps (truncate.ts): 2000 lines or 50KB, whichever is hit first.
const DEFAULT_MAX_LINES: usize = 2000;
const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// MIME detection by magic bytes (mirrors the M4 `--image` detection in cli.rs: png/jpeg/gif/webp).
fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
	if bytes.len() > 7 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
		Some("image/png")
	} else if bytes.len() > 2 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
		Some("image/jpeg")
	} else if bytes.len() > 3 && &bytes[..4] == b"GIF8" {
		Some("image/gif")
	} else if bytes.len() > 11 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
		Some("image/webp")
	} else {
		None
	}
}

fn b64(data: &[u8]) -> String {
	const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	let mut out = String::new();
	for chunk in data.chunks(3) {
		let b0 = chunk[0];
		let b1 = *chunk.get(1).unwrap_or(&0);
		let b2 = *chunk.get(2).unwrap_or(&0);
		out.push(T[(b0 >> 2) as usize] as char);
		out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
		out.push(if chunk.len() > 1 { T[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
		out.push(if chunk.len() > 2 { T[(b2 & 63) as usize] as char } else { '=' });
	}
	out
}

/// Upstream `formatSize`: whole bytes below 1KB, one decimal beyond.
fn format_size(bytes: usize) -> String {
	if bytes < 1024 {
		format!("{bytes}B")
	} else if bytes < 1024 * 1024 {
		format!("{}KB", to_fixed_1(bytes as f64 / 1024.0))
	} else {
		format!("{}MB", to_fixed_1(bytes as f64 / 1048576.0))
	}
}

/// JS `toFixed(1)`: ties round to the larger digit (1.25 → "1.3"; Rust's `{:.1}` would say "1.2").
fn to_fixed_1(x: f64) -> String {
	let n = (x * 10.0 + 0.5).floor() as i64;
	format!("{}.{}", n / 10, n % 10)
}

struct Truncation {
	content: String,
	truncated: bool,
	truncated_by: Option<&'static str>,
	total_lines: usize,
	total_bytes: usize,
	output_lines: usize,
	output_bytes: usize,
	first_line_exceeds_limit: bool,
}

/// Upstream `truncateHead`: keep whole leading lines under both caps; a first line alone over the
/// byte cap is reported via `first_line_exceeds_limit` with empty content.
fn truncate_head(content: &str) -> Truncation {
	let max_lines = DEFAULT_MAX_LINES;
	let max_bytes = DEFAULT_MAX_BYTES;
	let total_bytes = content.len();
	let lines: Vec<&str> = content.split('\n').collect();
	// Upstream `splitLinesForCounting`: split on \n, drop the trailing empty line.
	let total_lines = if content.ends_with('\n') { lines.len() - 1 } else { lines.len() };
	if total_lines <= max_lines && total_bytes <= max_bytes {
		return Truncation {
			content: content.to_string(),
			truncated: false,
			truncated_by: None,
			total_lines,
			total_bytes,
			output_lines: total_lines,
			output_bytes: total_bytes,
			first_line_exceeds_limit: false,
		};
	}
	if lines[0].len() > max_bytes {
		return Truncation {
			content: String::new(),
			truncated: true,
			truncated_by: Some("bytes"),
			total_lines,
			total_bytes,
			output_lines: 0,
			output_bytes: 0,
			first_line_exceeds_limit: true,
		};
	}
	let mut out: Vec<&str> = Vec::new();
	let mut out_bytes = 0;
	let mut truncated_by = "lines";
	// Upstream loops over the *counted* lines (trailing empty line already dropped).
	for (i, line) in lines.iter().take(total_lines.min(max_lines)).enumerate() {
		let line_bytes = line.len() + if i > 0 { 1 } else { 0 }; // +1 for the newline
		if out_bytes + line_bytes > max_bytes {
			truncated_by = "bytes";
			break;
		}
		out.push(line);
		out_bytes += line_bytes;
	}
	if out.len() >= max_lines && out_bytes <= max_bytes {
		truncated_by = "lines";
	}
	let text = out.join("\n");
	Truncation {
		content: text.clone(),
		truncated: true,
		truncated_by: Some(truncated_by),
		total_lines,
		total_bytes,
		output_lines: out.len(),
		output_bytes: text.len(),
		first_line_exceeds_limit: false,
	}
}

/// `details` for a truncated read: upstream's `TruncationResult` under the `truncation` key
/// (same fields and values; serde_json orders the object's keys itself).
fn truncation_details(t: &Truncation) -> Value {
	json!({
		"truncation": {
			"content": t.content,
			"truncated": t.truncated,
			"truncatedBy": t.truncated_by,
			"totalLines": t.total_lines,
			"totalBytes": t.total_bytes,
			"outputLines": t.output_lines,
			"outputBytes": t.output_bytes,
			"lastLinePartial": false,
			"firstLineExceedsLimit": t.first_line_exceeds_limit,
			"maxLines": DEFAULT_MAX_LINES,
			"maxBytes": DEFAULT_MAX_BYTES,
		}
	})
}

/// Upstream `resolveReadPathAsync`, without the macOS screenshot-Unicode fallbacks: strip a
/// leading `@`, expand a leading `~/`, resolve relative paths against the cwd.
fn resolve_read_path(path: &str) -> PathBuf {
	let p = path.strip_prefix('@').unwrap_or(path);
	if let Some(rest) = p.strip_prefix("~/") {
		if let Some(home) = std::env::var_os("HOME") {
			return Path::new(&home).join(rest);
		}
	}
	let p = Path::new(p);
	if p.is_absolute() {
		p.to_path_buf()
	} else {
		std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
	}
}

/// Shape a `std::fs` failure like the Node.js `error.message` the upstream tool rejects with
/// (the agent loop surfaces `error.message` verbatim as the tool result text).
fn node_fs_error(e: &std::io::Error, op: &str, path: &Path) -> String {
	use std::io::ErrorKind;
	match e.kind() {
		ErrorKind::NotFound => format!("ENOENT: no such file or directory, {op} '{}'", path.display()),
		ErrorKind::PermissionDenied => format!("EACCES: permission denied, {op} '{}'", path.display()),
		// Reachable from `write` (target is a directory); read() handles its EISDIR explicitly.
		ErrorKind::IsADirectory => format!("EISDIR: illegal operation on a directory, {op} '{}'", path.display()),
		_ => e.to_string(),
	}
}

/// Index clamping of `Array.prototype.slice`: truncate toward zero, negatives relative to length.
fn js_slice_bound(x: f64, len: usize) -> usize {
	let v = x.trunc();
	let len = len as f64;
	let r = if v < 0.0 { (len + v).max(0.0) } else { v.min(len) };
	r as usize
}

fn read(input: &Value) -> ExecResult {
	let Some(path) = input.get("path").and_then(Value::as_str) else {
		return ExecResult::err("read: missing string field `path`".into());
	};
	let abs = resolve_read_path(path);
	// Upstream checks readability first (fs.access R_OK); a directory is readable but the
	// subsequent open fails with EISDIR.
	match std::fs::metadata(&abs) {
		Err(e) => return ExecResult::err(node_fs_error(&e, "access", &abs)),
		Ok(md) if md.is_dir() => {
			return ExecResult::err(format!(
				"EISDIR: illegal operation on a directory, open '{}'",
				abs.display()
			));
		}
		Ok(_) => {}
	}
	let mut file = match std::fs::File::open(&abs) {
		Ok(f) => f,
		Err(e) => return ExecResult::err(node_fs_error(&e, "access", &abs)),
	};
	let mut bytes = Vec::new();
	if let Err(e) = std::io::Read::read_to_end(&mut file, &mut bytes) {
		return ExecResult::err(node_fs_error(&e, "open", &abs));
	}
	// Image: upstream returns a text note plus an image block; the block's data/mimeType/note
	// travel in `details` (ExecResult content stays text-only; WP-408 consumes the shape).
	if let Some(mime) = detect_image_mime(&bytes) {
		let note = format!("Read image file [{mime}]");
		return ExecResult {
			content: json!([{ "type": "text", "text": note.clone() }]),
			details: json!({ "type": "image", "data": b64(&bytes), "mimeType": mime, "note": note }),
			is_error: false,
			terminate: false,
		};
	}
	// Text: upstream's whole-file algorithm (read.ts `selectLinesFromBuffer` + `truncateHead`).
	// Invalid UTF-8 decodes lossy, like `Buffer.toString("utf-8")`.
	let text = String::from_utf8_lossy(&bytes).into_owned();
	let all_lines: Vec<&str> = text.split('\n').collect();
	let total_file_lines = all_lines.len();
	let offset = input.get("offset").and_then(Value::as_f64);
	let limit = input.get("limit").and_then(Value::as_f64);
	// 1-based offset → 0-based line: `offset ? Math.max(0, offset - 1) : 0`.
	let start_line = match offset {
		Some(o) if o != 0.0 => (o - 1.0).max(0.0),
		_ => 0.0,
	};
	let start_line_display = start_line + 1.0;
	if start_line >= total_file_lines as f64 {
		// Only reachable with a truthy offset (everything else starts at line 0 of a non-empty array).
		let shown = offset.expect("start_line >= total_file_lines implies a truthy offset");
		return ExecResult::err(format!(
			"Offset {shown} is beyond end of file ({total_file_lines} lines total)"
		));
	}
	// `allLines.slice(startLine, endLine)` with JS index clamping.
	let end_line = limit
		.map(|l| (start_line + l).min(total_file_lines as f64))
		.unwrap_or(total_file_lines as f64);
	let s = js_slice_bound(start_line, total_file_lines);
	let e = js_slice_bound(end_line, total_file_lines);
	let trunc = truncate_head(&all_lines[s..e].join("\n"));
	// A user-specified limit is honored before the caps; remaining lines earn a notice.
	let user_limited_lines = limit.map(|l| (start_line + l).min(total_file_lines as f64) - start_line);
	let (output_text, details) = if trunc.first_line_exceeds_limit {
		let first_line_size = format_size(all_lines[s].len());
		let t = format!(
			"[Line {start_line_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n '{start_line_display}p' {path} | head -c {DEFAULT_MAX_BYTES}]",
			format_size(DEFAULT_MAX_BYTES)
		);
		(t, truncation_details(&trunc))
	} else if trunc.truncated {
		let end_line_display = start_line_display + trunc.output_lines as f64 - 1.0;
		let next_offset = end_line_display + 1.0;
		let mut t = trunc.content.clone();
		let notice = if trunc.truncated_by == Some("lines") {
			format!(
				"\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={next_offset} to continue.]"
			)
		} else {
			format!(
				"\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines} ({} limit). Use offset={next_offset} to continue.]",
				format_size(DEFAULT_MAX_BYTES)
			)
		};
		t.push_str(&notice);
		(t, truncation_details(&trunc))
	} else {
		match user_limited_lines {
			Some(ull) if start_line + ull < total_file_lines as f64 => {
				let remaining = total_file_lines as f64 - (start_line + ull);
				let next_offset = start_line + ull + 1.0;
				(format!(
					"{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
					trunc.content
				),
				json!({}))
			}
			_ => (trunc.content, json!({})),
		}
	};
	ExecResult {
		content: json!([{ "type": "text", "text": output_text }]),
		details,
		is_error: false,
		terminate: false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::PathBuf;

	// Same minimal PNG the cli.rs attachment tests use (magic + a few extra bytes).
	const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

	fn tmpdir() -> PathBuf {
		let d = std::env::temp_dir().join(format!("pi-rust-read-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&d).unwrap();
		d
	}

	async fn read_tool(input: Value) -> ExecResult {
		execute("read", &input).await
	}

	async fn write_tool(input: Value) -> ExecResult {
		execute("write", &input).await
	}

	async fn edit_tool(input: Value) -> ExecResult {
		execute("edit", &input).await
	}

	async fn bash_tool(input: Value) -> ExecResult {
		execute("bash", &input).await
	}

	fn text(r: &ExecResult) -> String {
		r.content[0].get("text").and_then(Value::as_str).unwrap_or("").to_string()
	}

	#[tokio::test]
	async fn read_small_file_exact_output() {
		let dir = tmpdir();
		let p = dir.join("small.txt");
		std::fs::write(&p, "alpha\nbeta\ngamma").unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "alpha\nbeta\ngamma");
		assert_eq!(r.details, json!({}));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_preserves_trailing_newline() {
		let dir = tmpdir();
		let p = dir.join("trail.txt");
		std::fs::write(&p, "alpha\nbeta\n").unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert_eq!(text(&r), "alpha\nbeta\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_empty_file_yields_empty_text() {
		let dir = tmpdir();
		let p = dir.join("empty.txt");
		std::fs::write(&p, "").unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "");
		assert_eq!(r.details, json!({}));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_truncates_at_2000_lines_golden() {
		let dir = tmpdir();
		let p = dir.join("lines.txt");
		let all: String = (0..2001).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
		std::fs::write(&p, &all).unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		let body: String = (0..2000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
		let expected = format!("{body}\n\n[Showing lines 1-2000 of 2001. Use offset=2001 to continue.]");
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(body));
		assert_eq!(t["truncated"], json!(true));
		assert_eq!(t["truncatedBy"], json!("lines"));
		assert_eq!(t["totalLines"], json!(2001));
		assert_eq!(t["totalBytes"], json!(18899));
		assert_eq!(t["outputLines"], json!(2000));
		assert_eq!(t["outputBytes"], json!(18889));
		assert_eq!(t["lastLinePartial"], json!(false));
		assert_eq!(t["firstLineExceedsLimit"], json!(false));
		assert_eq!(t["maxLines"], json!(2000));
		assert_eq!(t["maxBytes"], json!(51200));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_truncates_at_50kb_golden() {
		let dir = tmpdir();
		let p = dir.join("kb.txt");
		// Mirrors upstream's `FIFTY_KB_OF_LINES`: 511 lines of 99 'y' plus one 100-'y' line
		// (the appended 'y' extends the last line) + trailing newline = exactly 50KB + 1 byte.
		let body = format!("{}\n{}", vec!["y".repeat(99); 511].join("\n"), "y".repeat(100));
		let big = body.clone() + "\n";
		assert_eq!(big.len(), 51201);
		std::fs::write(&p, &big).unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		// 50KB + 1 byte: truncated, yet every line fits, so upstream's truncateHead
		// reports truncatedBy "lines" and the notice carries no byte-limit clause.
		let expected = format!("{body}\n\n[Showing lines 1-512 of 513. Use offset=513 to continue.]");
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(body));
		assert_eq!(t["truncatedBy"], json!("lines"));
		assert_eq!(t["totalLines"], json!(512));
		assert_eq!(t["totalBytes"], json!(51201));
		assert_eq!(t["outputLines"], json!(512));
		assert_eq!(t["outputBytes"], json!(51200));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_truncates_by_bytes_midfile_golden() {
		let dir = tmpdir();
		let p = dir.join("z.txt");
		// 1000 lines of 60 'z' (60999 bytes): the byte cap breaks mid-file, so the
		// notice carries the "(50.0KB limit)" clause.
		let z60 = "z".repeat(60);
		let full = vec![z60.clone(); 1000].join("\n");
		std::fs::write(&p, &full).unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		let body = vec![z60; 839].join("\n");
		let expected = format!("{body}\n\n[Showing lines 1-839 of 1000 (50.0KB limit). Use offset=840 to continue.]");
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(body));
		assert_eq!(t["truncatedBy"], json!("bytes"));
		assert_eq!(t["totalLines"], json!(1000));
		assert_eq!(t["totalBytes"], json!(60999));
		assert_eq!(t["outputLines"], json!(839));
		assert_eq!(t["outputBytes"], json!(51178));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_first_line_over_byte_limit_golden() {
		let dir = tmpdir();
		let p = dir.join("bigline.txt");
		// 25601 'é' = 51202 UTF-8 bytes: the first line alone exceeds the 50KB limit.
		let first = "é".repeat(25601);
		std::fs::write(&p, format!("{first}\nsecond")).unwrap();
		let path = p.display().to_string();
		let r = read_tool(json!({"path": path})).await;
		assert!(!r.is_error);
		let expected = format!("[Line 1 is 50.0KB, exceeds 50.0KB limit. Use bash: sed -n '1p' {path} | head -c 51200]");
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(""));
		assert_eq!(t["truncatedBy"], json!("bytes"));
		assert_eq!(t["firstLineExceedsLimit"], json!(true));
		assert_eq!(t["totalLines"], json!(2));
		assert_eq!(t["totalBytes"], json!(51209));
		assert_eq!(t["outputLines"], json!(0));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_offset_limit_range_remaining_notice() {
		let dir = tmpdir();
		let p = dir.join("five.txt");
		std::fs::write(&p, "one\ntwo\nthree\nfour\nfive").unwrap();
		let r = read_tool(json!({"path": p.display().to_string(), "offset": 2, "limit": 2})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "two\nthree\n\n[2 more lines in file. Use offset=4 to continue.]");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_offset_limit_exact_end_no_notice() {
		let dir = tmpdir();
		let p = dir.join("five.txt");
		std::fs::write(&p, "one\ntwo\nthree\nfour\nfive").unwrap();
		let r = read_tool(json!({"path": p.display().to_string(), "offset": 5, "limit": 1})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "five");
		assert_eq!(r.details, json!({}));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_offset_beyond_end_upstream_error() {
		let dir = tmpdir();
		let p = dir.join("three.txt");
		std::fs::write(&p, "a\nb\nc").unwrap();
		let r = read_tool(json!({"path": p.display().to_string(), "offset": 10})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "Offset 10 is beyond end of file (3 lines total)");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_missing_file_upstream_error() {
		let dir = tmpdir();
		let p = dir.join("nope.txt");
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), format!("ENOENT: no such file or directory, access '{}'", p.display()));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_directory_upstream_error() {
		let dir = tmpdir();
		let r = read_tool(json!({"path": dir.display().to_string()})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), format!("EISDIR: illegal operation on a directory, open '{}'", dir.display()));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_unreadable_file_upstream_error() {
		// root bypasses file permissions; nothing to assert there.
		let is_root = std::process::Command::new("id").arg("-u")
			.output()
			.ok()
			.is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
		if is_root {
			return;
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let dir = tmpdir();
			let p = dir.join("secret.txt");
			std::fs::write(&p, "top secret").unwrap();
			std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0)).unwrap();
			let r = read_tool(json!({"path": p.display().to_string()})).await;
			assert!(r.is_error);
			assert_eq!(text(&r), format!("EACCES: permission denied, access '{}'", p.display()));
			std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
			std::fs::remove_dir_all(&dir).unwrap();
		}
	}

	#[tokio::test]
	async fn read_png_image_shape() {
		let dir = tmpdir();
		let p = dir.join("shot.png");
		std::fs::write(&p, PNG).unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "Read image file [image/png]");
		assert_eq!(r.details["type"], json!("image"));
		assert_eq!(r.details["data"], json!("iVBORw0KGgoBAgM="));
		assert_eq!(r.details["mimeType"], json!("image/png"));
		assert_eq!(r.details["note"], json!("Read image file [image/png]"));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_invalid_utf8_decodes_lossy() {
		// Upstream has no binary rejection path: Buffer.toString("utf-8") replaces
		// invalid bytes with U+FFFD and the file is served as text.
		let dir = tmpdir();
		let p = dir.join("bin.dat");
		std::fs::write(&p, [0x61, 0xFF, 0xFE, 0x62, 0x0A]).unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "a\u{FFFD}\u{FFFD}b\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_offset_zero_and_negative_read_from_start() {
		let dir = tmpdir();
		let p = dir.join("three.txt");
		std::fs::write(&p, "one\ntwo\nthree").unwrap();
		for offset in [0, -3] {
			let r = read_tool(json!({"path": p.display().to_string(), "offset": offset})).await;
			assert!(!r.is_error);
			assert_eq!(text(&r), "one\ntwo\nthree", "offset {offset}");
		}
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_limit_zero_yields_remaining_notice() {
		let dir = tmpdir();
		let p = dir.join("three.txt");
		std::fs::write(&p, "one\ntwo\nthree").unwrap();
		let r = read_tool(json!({"path": p.display().to_string(), "limit": 0})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "\n\n[3 more lines in file. Use offset=1 to continue.]");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn read_crlf_and_bom_preserved() {
		let dir = tmpdir();
		let p = dir.join("crlf.txt");
		std::fs::write(&p, "\u{FEFF}alpha\r\nbeta\r\n").unwrap();
		let r = read_tool(json!({"path": p.display().to_string()})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "\u{FEFF}alpha\r\nbeta\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_creates_file_exact_upstream_result() {
		let dir = tmpdir();
		let p = dir.join("new.txt");
		let r = write_tool(json!({"path": p.display().to_string(), "content": "hello"})).await;
		assert!(!r.is_error);
		// Upstream (write.ts): `Successfully wrote to ${path}` — the raw input path, and
		// `details: undefined` ({} here, the codebase convention for an empty details object).
		assert_eq!(text(&r), format!("Successfully wrote to {}", p.display()));
		assert_eq!(r.details, json!({}));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_overwrites_existing_file() {
		let dir = tmpdir();
		let p = dir.join("old.txt");
		std::fs::write(&p, "old content").unwrap();
		let r = write_tool(json!({"path": p.display().to_string(), "content": "new"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully wrote to {}", p.display()));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_creates_nested_parent_dirs() {
		let dir = tmpdir();
		let p = dir.join("a").join("b").join("c").join("deep.txt");
		let r = write_tool(json!({"path": p.display().to_string(), "content": "Nested content"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully wrote to {}", p.display()));
		assert!(p.parent().unwrap().is_dir());
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "Nested content");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_at_prefix_stripped_input_path_echoed() {
		let dir = tmpdir();
		let p = dir.join("at.txt");
		let raw = format!("@{}", p.display());
		let r = write_tool(json!({"path": raw.clone(), "content": "x"})).await;
		assert!(!r.is_error);
		// The @-prefix is stripped for the filesystem op, but the result echoes the raw input.
		assert_eq!(text(&r), format!("Successfully wrote to {raw}"));
		assert!(p.exists());
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "x");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_to_directory_eisdir_upstream_error() {
		let dir = tmpdir();
		let r = write_tool(json!({"path": dir.display().to_string(), "content": "x"})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), format!("EISDIR: illegal operation on a directory, open '{}'", dir.display()));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn write_read_only_target_eacces_upstream_error() {
		// root bypasses file permissions; nothing to assert there.
		let is_root = std::process::Command::new("id").arg("-u")
			.output()
			.ok()
			.is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
		if is_root {
			return;
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let dir = tmpdir();
			let p = dir.join("blocked.txt");
			std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
			let r = write_tool(json!({"path": p.display().to_string(), "content": "x"})).await;
			assert!(r.is_error);
			// Node's writeFile error message (op word "open"), resolved to the absolute path.
			assert_eq!(text(&r), format!("EACCES: permission denied, open '{}'", p.display()));
			std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
			std::fs::remove_dir_all(&dir).unwrap();
		}
	}

	#[tokio::test]
	async fn write_missing_fields_preserved_v0_error() {
		// PRESERVE: the field-validation message predates WP-502 and must not change.
		let dir = tmpdir();
		let p = dir.join("fields.txt");
		let r = write_tool(json!({"path": p.display().to_string()})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "write: needs string fields `path` and `content`");
		assert!(!p.exists());
		std::fs::remove_dir_all(&dir).unwrap();
	}

	// ---- WP-503: edit tool, mirroring the pinned upstream behavior table ----

	#[tokio::test]
	async fn edit_unique_single_exact_result_and_file() {
		let dir = tmpdir();
		let p = dir.join("edit-test.txt");
		std::fs::write(&p, "Hello, world!").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": [{"oldText": "world", "newText": "testing"}]})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully replaced 1 block(s) in {path}."));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "Hello, testing!");
		// Upstream EditToolDetails: {diff, patch, firstChangedLine}.
		let diff = r.details["diff"].as_str().expect("details.diff is a string");
		assert!(diff.contains("testing"));
		let patch = r.details["patch"].as_str().expect("details.patch is a string");
		assert!(patch.contains("--- ") && patch.contains("+++ ") && patch.contains("@@"));
		assert!(patch.contains("-Hello, world!") && patch.contains("+Hello, testing!"));
		assert_eq!(r.details["firstChangedLine"], json!(1));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_no_match_exact_error_file_unchanged() {
		let dir = tmpdir();
		let p = dir.join("edit-test.txt");
		std::fs::write(&p, "Hello, world!").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": [{"oldText": "nonexistent", "newText": "testing"}]})).await;
		assert!(r.is_error);
		assert_eq!(
			text(&r),
			format!("Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.")
		);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "Hello, world!");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_missing_file_enoent_wrapped() {
		let dir = tmpdir();
		let p = dir.join("missing.txt");
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": [{"oldText": "hello", "newText": "world"}]})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), format!("Could not edit file: {path}. Error code: ENOENT."));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_ambiguous_single_exact_error() {
		let dir = tmpdir();
		let p = dir.join("edit-test.txt");
		std::fs::write(&p, "foo foo foo").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": [{"oldText": "foo", "newText": "bar"}]})).await;
		assert!(r.is_error);
		assert_eq!(
			text(&r),
			format!("Found 3 occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.")
		);
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_multi_disjoint_regions_one_call() {
		let dir = tmpdir();
		let p = dir.join("edit-multi.txt");
		std::fs::write(&p, "alpha\nbeta\ngamma\ndelta\n").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({
			"path": path,
			"edits": [{"oldText": "alpha\n", "newText": "ALPHA\n"}, {"oldText": "gamma\n", "newText": "GAMMA\n"}]
		})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully replaced 2 block(s) in {path}."));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "ALPHA\nbeta\nGAMMA\ndelta\n");
		let diff = r.details["diff"].as_str().unwrap();
		assert!(diff.contains("ALPHA") && diff.contains("GAMMA"));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_multi_matched_against_original_not_incremental() {
		let dir = tmpdir();
		let p = dir.join("edit-multi-original.txt");
		std::fs::write(&p, "foo\nbar\nbaz\n").unwrap();
		let r = edit_tool(json!({
			"path": p.display().to_string(),
			"edits": [{"oldText": "foo\n", "newText": "foo bar\n"}, {"oldText": "bar\n", "newText": "BAR\n"}]
		})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "foo bar\nBAR\nbaz\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_empty_edits_exact_error() {
		let dir = tmpdir();
		let p = dir.join("edit-empty.txt");
		std::fs::write(&p, "hello\nworld\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": []})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "Edit tool input is invalid. edits must contain at least one replacement.");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_overlapping_regions_exact_error_file_unchanged() {
		let dir = tmpdir();
		let p = dir.join("edit-overlap.txt");
		std::fs::write(&p, "one\ntwo\nthree\n").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({
			"path": path,
			"edits": [{"oldText": "one\ntwo\n", "newText": "ONE\nTWO\n"}, {"oldText": "two\nthree\n", "newText": "TWO\nTHREE\n"}]
		})).await;
		assert!(r.is_error);
		assert_eq!(
			text(&r),
			format!("edits[0] and edits[1] overlap in {path}. Merge them into one edit or target disjoint regions.")
		);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "one\ntwo\nthree\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_no_partial_apply_when_one_edit_fails() {
		let dir = tmpdir();
		let p = dir.join("edit-no-partial.txt");
		let original = "alpha\nbeta\ngamma\n";
		std::fs::write(&p, original).unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({
			"path": path,
			"edits": [{"oldText": "alpha\n", "newText": "ALPHA\n"}, {"oldText": "missing\n", "newText": "MISSING\n"}]
		})).await;
		assert!(r.is_error);
		assert_eq!(
			text(&r),
			format!("Could not find edits[1] in {path}. The oldText must match exactly including all whitespace and newlines.")
		);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_read_only_file_eacces_wrapped() {
		let is_root = std::process::Command::new("id").arg("-u")
			.output()
			.ok()
			.is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
		if is_root {
			return;
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let dir = tmpdir();
			let p = dir.join("edit-readonly.txt");
			std::fs::write(&p, "hello\n").unwrap();
			std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
			let path = p.display().to_string();
			let r = edit_tool(json!({"path": path, "edits": [{"oldText": "hello", "newText": "world"}]})).await;
			assert!(r.is_error);
			assert_eq!(text(&r), format!("Could not edit file: {path}. Error code: EACCES."));
			assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
			std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
			std::fs::remove_dir_all(&dir).unwrap();
		}
	}

	#[tokio::test]
	async fn edit_crlf_line_endings_preserved() {
		let dir = tmpdir();
		let p = dir.join("crlf-preserve.txt");
		std::fs::write(&p, "first\r\nsecond\r\nthird\r\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "first\r\nREPLACED\r\nthird\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_lf_line_endings_preserved() {
		let dir = tmpdir();
		let p = dir.join("lf-preserve.txt");
		std::fs::write(&p, "first\nsecond\nthird\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "first\nREPLACED\nthird\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_duplicate_across_crlf_lf_variants() {
		let dir = tmpdir();
		let p = dir.join("mixed-endings.txt");
		std::fs::write(&p, "hello\r\nworld\r\n---\r\nhello\nworld\n").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": [{"oldText": "hello\nworld\n", "newText": "replaced\n"}]})).await;
		assert!(r.is_error);
		assert_eq!(
			text(&r),
			format!("Found 2 occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.")
		);
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_bom_preserved() {
		let dir = tmpdir();
		let p = dir.join("bom-test.txt");
		std::fs::write(&p, "\u{FEFF}first\r\nsecond\r\nthird\r\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "second\n", "newText": "REPLACED\n"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "\u{FEFF}first\r\nREPLACED\r\nthird\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_bom_and_crlf_in_multi_edit_mode() {
		let dir = tmpdir();
		let p = dir.join("bom-crlf-multi.txt");
		std::fs::write(&p, "\u{FEFF}first\r\nsecond\r\nthird\r\nfourth\r\n").unwrap();
		let r = edit_tool(json!({
			"path": p.display().to_string(),
			"edits": [{"oldText": "second\n", "newText": "SECOND\n"}, {"oldText": "fourth\n", "newText": "FOURTH\n"}]
		})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "\u{FEFF}first\r\nSECOND\r\nthird\r\nFOURTH\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_inserted_lines_get_crlf_in_crlf_file() {
		let dir = tmpdir();
		let p = dir.join("crlf-bytes.txt");
		std::fs::write(&p, "first\r\nsecond\r\nthird").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "second\nthird", "newText": "2a\r\n2b\nthird!"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"first\r\n2a\r\n2b\r\nthird!");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_inserted_lines_get_lf_in_lf_file() {
		let dir = tmpdir();
		let p = dir.join("lf-bytes.txt");
		std::fs::write(&p, "first\nsecond\nthird").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "second\nthird", "newText": "2a\r\n2b\nthird!"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"first\n2a\n2b\nthird!");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_mixed_lf_first_line_8544() {
		let dir = tmpdir();
		let p = dir.join("mixed-lf-first.txt");
		std::fs::write(&p, "a\nb\r\nX = 1\r\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "X = 1", "newText": "X = 2"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"a\nb\r\nX = 2\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_mixed_crlf_first_line_8544() {
		let dir = tmpdir();
		let p = dir.join("mixed-crlf-first.txt");
		std::fs::write(&p, "a\r\nb\nc").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "c", "newText": "C"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"a\r\nb\nC");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_lone_cr_in_untouched_line_8544() {
		let dir = tmpdir();
		let p = dir.join("lone-cr.txt");
		std::fs::write(&p, "a\rb\nX = 1\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": [{"oldText": "X = 1", "newText": "X = 2"}]})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"a\rb\nX = 2\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_mixed_endings_multi_edit_inserted_lines_8544() {
		let dir = tmpdir();
		let p = dir.join("mixed-multi.txt");
		std::fs::write(&p, "lf1\nlf2\ncr1\r\ncr2\r\n").unwrap();
		let r = edit_tool(json!({
			"path": p.display().to_string(),
			"edits": [{"oldText": "lf1\n", "newText": "A\nB\n"}, {"oldText": "cr1\n", "newText": "C\nD\n"}]
		})).await;
		assert!(!r.is_error);
		assert_eq!(std::fs::read(&p).unwrap(), b"A\nB\nlf2\nC\r\nD\r\ncr2\r\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_legacy_top_level_old_new_text() {
		// Upstream prepareArguments: top-level oldText/newText fold into edits.
		let dir = tmpdir();
		let p = dir.join("legacy.txt");
		std::fs::write(&p, "before\n").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "oldText": "before", "newText": "after"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully replaced 1 block(s) in {path}."));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "after\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_stringified_edits_parsed() {
		let dir = tmpdir();
		let p = dir.join("string-edits.txt");
		std::fs::write(&p, "before\n").unwrap();
		let path = p.display().to_string();
		let r = edit_tool(json!({"path": path, "edits": "[{\"oldText\":\"before\",\"newText\":\"after\"}]"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully replaced 1 block(s) in {path}."));
		assert_eq!(std::fs::read_to_string(&p).unwrap(), "after\n");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_stringified_edits_invalid_json_is_invalid_input() {
		let dir = tmpdir();
		let p = dir.join("bad-string.txt");
		std::fs::write(&p, "before\n").unwrap();
		let r = edit_tool(json!({"path": p.display().to_string(), "edits": "not json"})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "Edit tool input is invalid. edits must contain at least one replacement.");
		std::fs::remove_dir_all(&dir).unwrap();
	}

	#[tokio::test]
	async fn edit_missing_path_field_house_error() {
		let r = edit_tool(json!({"edits": [{"oldText": "a", "newText": "b"}]})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "edit: missing string field `path`");
	}

	// ---- WP-504: bash tool, mirroring the pinned upstream behavior (bash.ts @ 085c84e) ----

	/// The `fullOutputPath` details entry (truncated output only).
	fn full_output_path(r: &ExecResult) -> Option<PathBuf> {
		r.details.get("fullOutputPath").and_then(Value::as_str).map(PathBuf::from)
	}

	/// VmRSS in kB (Linux only; the memory-bounded check skips elsewhere).
	fn vm_rss_kb() -> Option<u64> {
		#[cfg(target_os = "linux")]
		{
			let status = std::fs::read_to_string("/proc/self/status").ok()?;
			status.lines()
				.find_map(|l| l.strip_prefix("VmRSS:"))
				.and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
		}
		#[cfg(not(target_os = "linux"))]
		{
			None
		}
	}

	#[tokio::test]
	async fn bash_success_exact_upstream_output() {
		let r = bash_tool(json!({"command": "echo hi"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "hi\n");
		assert_eq!(r.details, json!({}));
	}

	#[tokio::test]
	async fn bash_no_output_placeholder() {
		let r = bash_tool(json!({"command": "true"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "(no output)");
		assert_eq!(r.details, json!({}));
	}

	#[tokio::test]
	async fn bash_stdout_stderr_interleaved_by_arrival() {
		// Sleeps make the arrival order deterministic (each write is consumed before the
		// next one): the contract under test is one merged accumulator in arrival order,
		// without a `[stderr]` block.
		let r = bash_tool(json!({"command": "printf 'a\n'; sleep 0.2; printf 'x\n' >&2; sleep 0.2; printf 'b\n'"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), "a\nx\nb\n");
		assert_eq!(r.details, json!({}));
	}

	#[tokio::test]
	async fn bash_nonzero_exit_with_stderr_golden() {
		let r = bash_tool(json!({"command": "echo out; echo err >&2; exit 3"})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "out\nerr\n\n\nCommand exited with code 3");
		assert_eq!(r.details, json!({}));
	}

	#[tokio::test]
	async fn bash_signal_killed_exit_137_golden() {
		let r = bash_tool(json!({"command": "kill -9 $$"})).await;
		assert!(r.is_error);
		// Upstream resolves a signal death to 128 + the signal number.
		assert_eq!(text(&r), "(no output)\n\nCommand exited with code 137");
	}

	#[tokio::test]
	async fn bash_timeout_kills_sleep_fast() {
		let started = std::time::Instant::now();
		let r = bash_tool(json!({"command": "sleep 5", "timeout": 1})).await;
		let elapsed = started.elapsed();
		assert!(r.is_error);
		assert_eq!(text(&r), "Command timed out after 1 seconds");
		assert!(elapsed >= std::time::Duration::from_secs(1), "killed before the deadline: {elapsed:?}");
		assert!(elapsed < std::time::Duration::from_secs(4), "timeout kill not fast: {elapsed:?}");
	}

	#[tokio::test]
	async fn bash_timeout_reports_output_so_far() {
		let r = bash_tool(json!({"command": "echo started; sleep 30", "timeout": 1})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "started\n\n\nCommand timed out after 1 seconds");
	}

	#[tokio::test]
	async fn bash_timeout_invalid_values_upstream_error() {
		for timeout in [json!(0), json!(-5), json!("5")] {
			let r = bash_tool(json!({"command": "echo x", "timeout": timeout})).await;
			assert!(r.is_error, "timeout {timeout}");
			assert_eq!(text(&r), "Invalid timeout: must be a finite number of seconds", "timeout {timeout}");
		}
	}

	#[tokio::test]
	async fn bash_timeout_invalid_max_upstream_error() {
		let r = bash_tool(json!({"command": "echo x", "timeout": 2147484})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "Invalid timeout: maximum is 2147483.647 seconds");
	}

	#[tokio::test]
	async fn bash_truncation_10mb_lines_golden() {
		let rss_before = vm_rss_kb();
		let r = bash_tool(json!({"command": "yes x | head -n 5242880"})).await;
		let rss_after = vm_rss_kb();
		assert!(!r.is_error);
		// 5242880 "x" lines = exactly 10MB: the tail keeps the last 2000 lines.
		let body: String = (0..2000).map(|_| "x").collect::<Vec<_>>().join("\n");
		let full = full_output_path(&r).expect("fullOutputPath when truncated");
		let expected = format!("{body}\n\n[Showing lines 5240881-5242880 of 5242880. Full output: {}]", full.display());
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(body));
		assert_eq!(t["truncated"], json!(true));
		assert_eq!(t["truncatedBy"], json!("lines"));
		assert_eq!(t["totalLines"], json!(5242880));
		assert_eq!(t["totalBytes"], json!(10485760));
		assert_eq!(t["outputLines"], json!(2000));
		assert_eq!(t["outputBytes"], json!(3999));
		assert_eq!(t["lastLinePartial"], json!(false));
		assert_eq!(t["firstLineExceedsLimit"], json!(false));
		assert_eq!(t["maxLines"], json!(2000));
		assert_eq!(t["maxBytes"], json!(51200));
		assert_eq!(r.details["fullOutputPath"], json!(full.to_string_lossy()));
		// The complete output went to the temp file, not into the result.
		let file_name = full.file_name().unwrap().to_string_lossy().into_owned();
		assert!(file_name.starts_with("pi-bash-") && file_name.ends_with(".log"), "{file_name}");
		assert_eq!(std::fs::metadata(&full).unwrap().len(), 10485760);
		std::fs::remove_file(&full).unwrap();
		// Memory bounded: the capture holds a rolling tail, not the 10MB.
		if let (Some(before), Some(after)) = (rss_before, rss_after) {
			assert!(
				after.saturating_sub(before) < 15_000,
				"RSS grew {before} -> {after} kB; the bash capture must stay bounded"
			);
		}
	}

	#[tokio::test]
	async fn bash_truncation_10mb_single_line_golden() {
		let r = bash_tool(json!({"command": "yes x | head -c 20971520 | tr -d '\n'"})).await;
		assert!(!r.is_error);
		// One 10MB line: the tail keeps its last 50KB (partial first line).
		let body = "x".repeat(51200);
		let full = full_output_path(&r).expect("fullOutputPath when truncated");
		let expected = format!("{body}\n\n[Showing last 50.0KB of line 1 (line is 10.0MB). Full output: {}]", full.display());
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["content"], json!(body));
		assert_eq!(t["truncatedBy"], json!("bytes"));
		assert_eq!(t["totalLines"], json!(1));
		assert_eq!(t["totalBytes"], json!(10485760));
		assert_eq!(t["outputLines"], json!(1));
		assert_eq!(t["outputBytes"], json!(51200));
		assert_eq!(t["lastLinePartial"], json!(true));
		assert_eq!(std::fs::metadata(&full).unwrap().len(), 10485760);
		std::fs::remove_file(&full).unwrap();
	}

	#[tokio::test]
	async fn bash_truncation_bytes_limit_clause_golden() {
		let r = bash_tool(json!({"command": "yes aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa | head -n 3000"})).await;
		assert!(!r.is_error);
		// 3000 lines of 40 'a' (123000 bytes): the byte cap breaks the tail first.
		let body: String = (0..1248).map(|_| "a".repeat(40)).collect::<Vec<_>>().join("\n");
		let full = full_output_path(&r).expect("fullOutputPath when truncated");
		let expected = format!("{body}\n\n[Showing lines 1753-3000 of 3000 (50.0KB limit). Full output: {}]", full.display());
		assert_eq!(text(&r), expected);
		let t = &r.details["truncation"];
		assert_eq!(t["truncatedBy"], json!("bytes"));
		assert_eq!(t["totalLines"], json!(3000));
		assert_eq!(t["totalBytes"], json!(123000));
		assert_eq!(t["outputLines"], json!(1248));
		assert_eq!(t["outputBytes"], json!(51167));
		std::fs::remove_file(&full).unwrap();
	}

	#[tokio::test]
	async fn bash_schema_upstream_description_and_timeout_param() {
		let bash = schemas().into_iter().find(|s| s["name"] == "bash").unwrap();
		assert_eq!(
			bash["description"],
			"Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds."
		);
		assert_eq!(bash["parameters"]["properties"]["command"]["description"], "Shell command to execute");
		assert_eq!(bash["parameters"]["properties"]["timeout"]["type"], "number");
		assert_eq!(bash["parameters"]["properties"]["timeout"]["description"], "Timeout in seconds (optional, no default timeout)");
		assert_eq!(bash["parameters"]["required"], json!(["command"]));
	}

	#[tokio::test]
	async fn bash_missing_command_preserved_v0_error() {
		// PRESERVE: the field-validation message predates WP-504 and must not change.
		let r = bash_tool(json!({})).await;
		assert!(r.is_error);
		assert_eq!(text(&r), "bash: missing string field `command`");
	}

	#[tokio::test]
	async fn preserve_tool_registration_and_write_dispatch() {
		// PRESERVE: WP-503 adds edit to schemas()/is_builtin()/execute(); the existing
		// entries and the write dispatch (pinned by WP-502) must be unchanged.
		let names: Vec<String> =
			schemas().into_iter().map(|s| s["name"].as_str().unwrap().to_string()).collect();
		assert_eq!(names, vec!["bash", "read", "write", "edit"]);
		assert!(is_builtin("bash") && is_builtin("read") && is_builtin("write") && is_builtin("edit"));
		assert!(!is_builtin("nope"));
		let dir = tmpdir();
		let p = dir.join("preserve.txt");
		let r = write_tool(json!({"path": p.display().to_string(), "content": "hi"})).await;
		assert!(!r.is_error);
		assert_eq!(text(&r), format!("Successfully wrote to {}", p.display()));
		std::fs::remove_dir_all(&dir).unwrap();
	}

	// WP-506: the built-in schemas must equal the upstream-generated schemas
	// (golden files from the pinned upstream, tests/fixtures/tool-schemas/).
	#[test]
	fn builtin_schemas_match_upstream_goldens() {
		for name in ["bash", "read", "write", "edit"] {
			let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/tool-schemas/{name}.json"));
			let golden: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
			let actual = schemas()
				.into_iter()
				.find(|s| s["name"] == name)
				.unwrap_or_else(|| panic!("built-in {name} missing from schemas()"));
			assert_eq!(actual, golden, "built-in {name} schema diverges from upstream");
		}
	}

	// WP-506: upstream registry precedence — extension tools shadow built-ins by
	// name (shadowed entry keeps its position), new extension tools append.
	#[test]
	fn merge_with_extensions_shadowing() {
		let ext = vec![
			json!({"name":"read","description":"shadow","parameters":{"type":"object","properties":{},"required":[]}}),
			json!({"name":"custom","description":"extra","parameters":{"type":"object","properties":{},"required":[]}}),
		];
		let merged = merge_with_extensions(schemas(), ext);
		let names: Vec<&str> = merged.iter().filter_map(|t| t["name"].as_str()).collect();
		assert_eq!(names, vec!["bash", "read", "write", "edit", "custom"], "shadowed read keeps its position, custom appends");
		assert_eq!(merged[1]["description"], "shadow", "extension tool shadows the built-in read");
		// Unchanged built-ins stay intact.
		let plain = merge_with_extensions(schemas(), vec![]);
		assert_eq!(plain, schemas());
	}
}

/// Upstream `write` (write.ts): resolve the path like the read tool, `mkdir` the parent dir
/// (recursive), `writeFile` the content, and echo the *raw input* path in the success text.
/// Failures surface as Node-shaped `error.message`s (op words: `mkdir` for the parent dir,
/// `open` for the file write — Node's writeFile reports under `open`).
fn write(input: &Value) -> ExecResult {
	let (Some(path), Some(content)) = (
		input.get("path").and_then(Value::as_str),
		input.get("content").and_then(Value::as_str),
	) else {
		return ExecResult::err("write: needs string fields `path` and `content`".into());
	};
	let abs = resolve_read_path(path);
	if let Some(dir) = abs.parent().filter(|d| !d.as_os_str().is_empty()) {
		if let Err(e) = std::fs::create_dir_all(dir) {
			return ExecResult::err(node_fs_error(&e, "mkdir", dir));
		}
	}
	match std::fs::write(&abs, content) {
		Ok(()) => ExecResult::text(format!("Successfully wrote to {path}")),
		Err(e) => ExecResult::err(node_fs_error(&e, "open", &abs)),
	}
}

/// Upstream `prepareEditArguments`: some models send `edits` as a JSON string or a single
/// edit object, or keep the legacy top-level `oldText`/`newText` pair. All fold into the
/// `edits` array. `None` means `validateEditInput` would reject: no non-array or an
/// empty array.
fn prepare_edit_edits(input: &Value) -> Option<Vec<Value>> {
	fn is_single_edit(v: &Value) -> bool {
		v.is_object()
			&& v.get("oldText").is_some_and(Value::is_string)
			&& v.get("newText").is_some_and(Value::is_string)
	}
	let mut edits = input.get("edits").cloned().unwrap_or(Value::Null);
	if let Some(s) = edits.as_str() {
		if let Ok(parsed) = serde_json::from_str::<Value>(s) {
			if parsed.is_array() || is_single_edit(&parsed) {
				edits = if parsed.is_array() { parsed } else { json!([parsed]) };
			}
		}
	}
	if is_single_edit(&edits) {
		edits = json!([edits]);
	}
	if let (Some(old_text), Some(new_text)) = (
		input.get("oldText").and_then(Value::as_str),
		input.get("newText").and_then(Value::as_str),
	) {
		let mut arr = edits.as_array().cloned().unwrap_or_default();
		arr.push(json!({"oldText": old_text, "newText": new_text}));
		edits = Value::Array(arr);
	}
	let arr = edits.as_array()?;
	(!arr.is_empty()).then(|| arr.clone())
}

/// Upstream `fs.access(R_OK|W_OK)` failure message: `Could not edit file: {path}. Error
/// code: {code}.` (the raw input path; uncoded errors surface as `Error: {message}`).
fn edit_access_error(e: &std::io::Error, path: &str) -> String {
	use std::io::ErrorKind;
	let message = match e.kind() {
		ErrorKind::NotFound => "Error code: ENOENT".to_string(),
		ErrorKind::PermissionDenied => "Error code: EACCES".to_string(),
		ErrorKind::IsADirectory => "Error code: EISDIR".to_string(),
		_ => format!("Error: {e}"),
	};
	format!("Could not edit file: {path}. {message}.")
}

/// Upstream `edit` (edit.ts): access → read → BOM strip → LF-normalize → apply edits
/// (all against the original content) → diff before the write → splice into the original
/// line endings → write. Result text echoes the raw input path; `details` is upstream's
/// `EditToolDetails` {diff, patch, firstChangedLine}.
fn edit(input: &Value) -> ExecResult {
	let Some(path) = input.get("path").and_then(Value::as_str) else {
		return ExecResult::err("edit: missing string field `path`".into());
	};
	let Some(edits_raw) = prepare_edit_edits(input) else {
		return ExecResult::err(
			"Edit tool input is invalid. edits must contain at least one replacement.".into(),
		);
	};
	let mut edits: Vec<(String, String)> = Vec::with_capacity(edits_raw.len());
	for (i, e) in edits_raw.iter().enumerate() {
		let (Some(old_text), Some(new_text)) = (
			e.get("oldText").and_then(Value::as_str),
			e.get("newText").and_then(Value::as_str),
		) else {
			return ExecResult::err(format!("edit: edits[{i}] needs string fields `oldText` and `newText`"));
		};
		edits.push((old_text.to_string(), new_text.to_string()));
	}
	let abs = resolve_read_path(path);
	// Upstream checks R_OK|W_OK before reading; the write probe below covers W_OK so a
	// read-only file rejects with the same wrapped message before anything is read.
	let mut file = match std::fs::File::open(&abs) {
		Ok(f) => f,
		Err(e) => return ExecResult::err(edit_access_error(&e, path)),
	};
	let mut bytes = Vec::new();
	if let Err(e) = std::io::Read::read_to_end(&mut file, &mut bytes) {
		return ExecResult::err(node_fs_error(&e, "read", &abs));
	}
	if let Err(e) = std::fs::OpenOptions::new().write(true).open(&abs) {
		return ExecResult::err(edit_access_error(&e, path));
	}
	// Upstream buffer.toString("utf-8"); invalid bytes decode lossy, like the read tool.
	let raw_content = String::from_utf8_lossy(&bytes).into_owned();
	// Strip BOM before matching: the model will not include an invisible BOM in oldText.
	let (bom, content) = edit_diff::split_bom(&raw_content);
	let normalized_content = edit_diff::normalize_to_lf(content);
	let applied = match edit_diff::apply_edits_to_normalized_content(&normalized_content, &edits, path) {
		Ok(a) => a,
		Err(msg) => return ExecResult::err(msg),
	};
	// Diff before writing, so diff computation cannot fail after the write.
	let diff_out = edit_diff::generate_edit_diff(path, &applied.base_content, &applied.new_content);
	// Splice into the original text, so untouched lines keep their line endings. Without any CR
	// the original is the normalized text, and the normalized result is already final.
	let final_content = if content == normalized_content {
		format!("{bom}{}", applied.new_content)
	} else {
		format!("{bom}{}", edit_diff::apply_replacements_to_original(content, &applied.replacements))
	};
	if let Err(e) = std::fs::write(&abs, &final_content) {
		return ExecResult::err(node_fs_error(&e, "open", &abs));
	}
	let mut details = json!({"diff": diff_out.diff, "patch": diff_out.patch});
	if let Some(line) = diff_out.first_changed_line {
		details["firstChangedLine"] = json!(line);
	}
	ExecResult {
		content: json!([{"type":"text","text":format!("Successfully replaced {} block(s) in {path}.", edits.len())}]),
		details,
		is_error: false,
		terminate: false,
	}
}
