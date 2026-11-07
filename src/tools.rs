use serde_json::{json, Value};
use tokio::process::Command;

/// v0 built-in tools: `bash`, `read`, `write` (no `edit` in v0).
pub fn schemas() -> Vec<Value> {
    vec![
        json!({"name":"bash","description":"Run a shell command (bash -c) and return its output","parameters":{"type":"object","properties":{"command":{"type":"string","description":"Command to run"}},"required":["command"]}}),
        json!({"name":"read","description":"Read a text file, optionally a line range (1-based)","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"]}}),
        json!({"name":"write","description":"Write content to a file, creating parent directories","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}),
    ]
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "bash" | "read" | "write")
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
        _ => ExecResult::err(format!("unknown built-in tool: {name}")),
    }
}

async fn bash(input: &Value) -> ExecResult {
    let Some(command) = input.get("command").and_then(Value::as_str) else {
        return ExecResult::err("bash: missing string field `command`".into());
    };
    let Ok(out) = Command::new("bash").arg("-c").arg(command).output().await else {
        return ExecResult::err("bash: failed to start".into());
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut text = stdout.into_owned();
    if !stderr.trim().is_empty() {
        text.push_str(&format!("\n[stderr]\n{stderr}"));
    }
    if out.status.success() {
        ExecResult { content: json!([{"type":"text","text":text}]), details: json!({"exitCode":0}), is_error: false, terminate: false }
    } else {
        let code = out.status.code().unwrap_or(-1);
        ExecResult { content: json!([{"type":"text","text":format!("exit {code}\n{text}")}]), details: json!({"exitCode":code}), is_error: true, terminate: false }
    }
}

fn read(input: &Value) -> ExecResult {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return ExecResult::err("read: missing string field `path`".into());
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return ExecResult::err(format!("read {path}: no such file or not readable"));
    };
    let mut lines: Vec<&str> = data.lines().collect();
    if let Some(off) = input.get("offset").and_then(Value::as_u64) {
        let off = off as usize;
        if off > 0 {
            lines = lines.split_off(off.saturating_sub(1));
        }
    }
    if let Some(lim) = input.get("limit").and_then(Value::as_u64) {
        lines.truncate(lim as usize);
    }
    ExecResult::text(lines.join("\n"))
}

fn write(input: &Value) -> ExecResult {
    let (Some(path), Some(content)) = (
        input.get("path").and_then(Value::as_str),
        input.get("content").and_then(Value::as_str),
    ) else {
        return ExecResult::err("write: needs string fields `path` and `content`".into());
    };
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && std::fs::create_dir_all(parent).is_err() {
            return ExecResult::err(format!("write {path}: cannot create parent dir"));
        }
    }
    match std::fs::write(p, content) {
        Ok(()) => ExecResult::text(format!("Wrote {} bytes to {path}", content.len())),
        Err(_) => ExecResult::err(format!("write {path}: write failed")),
    }
}
