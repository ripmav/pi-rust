use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// pi-rust v0 session file: JSONL of this project's OWN minimal entry shapes
/// (`user`, `assistant`, `tool_result`, `session_info`, plus verbatim sidecar
/// entries from `session_append_entry`). This is NOT upstream session-file
/// compatible — no cross-format claim in v0.
pub struct Session {
    pub path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    name: Option<String>,
    entries: Vec<Value>,
    messages: Vec<Value>, // AgentMessage transcript (pi-ai message shapes)
    file: File,
    next_id: u64,
}

impl Inner {
    fn new_id(&mut self) -> String {
        let id = format!("e{}", self.next_id);
        self.next_id += 1;
        id
    }

    fn write_line(file: &mut File, v: &Value) {
        let _ = writeln!(file, "{}", v.to_string().trim_end());
        let _ = file.flush();
    }

    /// Appends an AgentMessage to the transcript and a matching v0 entry to the
    /// file. Returns the entry id.
    fn append_message(&mut self, msg: &Value) -> String {
        let id = self.new_id();
        let ts = now_ms();
        let entry = match msg.get("role").and_then(Value::as_str) {
            Some("user") => json!({"type":"user","id":id,"timestamp":ts,"text":extract_text(msg)}),
            Some("assistant") => json!({"type":"assistant","id":id,"timestamp":ts,"message":msg}),
            Some("toolResult") => {
                let mut e = json!({"type":"tool_result","id":id,"timestamp":ts,"toolCallId":msg.get("toolCallId").cloned().unwrap_or(Value::Null),
                    "toolName":msg.get("toolName").cloned().unwrap_or(Value::Null),"content":msg.get("content").cloned().unwrap_or(json!([])),
                    "isError":msg.get("isError").cloned().unwrap_or(json!(false))});
                if let Some(d) = msg.get("details") { e["details"] = d.clone(); }
                e
            }
            other => json!({"type":"custom_message","id":id,"timestamp":ts,"role":other.unwrap_or("unknown"),"message":msg}),
        };
        self.messages.push(msg.clone());
        self.entries.push(entry.clone());
        Self::write_line(&mut self.file, &entry);
        id
    }

    fn append_entry(&mut self, entry: Value) {
        self.entries.push(entry.clone());
        Self::write_line(&mut self.file, &entry);
    }

    fn set_name(&mut self, name: Option<String>) {
        if name.as_deref() == self.name.as_deref() {
            return;
        }
        self.name = name.clone();
        let entry = json!({"type":"session_info","id":self.new_id(),"timestamp":now_ms(),"name":name});
        self.entries.push(entry.clone());
        Self::write_line(&mut self.file, &entry);
    }

    fn dump(&self) -> Value {
        json!({"name":self.name,"entries":self.entries,"messages":self.messages})
    }
}

impl Session {
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).ok(); }
        let file = OpenOptions::new().create(true).append(true).open(&path)
            .with_context(|| format!("opening session file {}", path.display()))?;
        Ok(Self { path, inner: Mutex::new(Inner { name: None, entries: Vec::new(), messages: Vec::new(), file, next_id: 1 }) })
    }

    pub fn append_message(&self, msg: &Value) -> String { self.inner.lock().unwrap().append_message(msg) }

    /// Persists a sidecar `session_append_entry` entry verbatim.
    pub fn append_entry(&self, entry: Value) { self.inner.lock().unwrap().append_entry(entry); }

    pub fn set_name(&self, name: Option<String>) { self.inner.lock().unwrap().set_name(name); }

    pub fn transcript(&self) -> Vec<Value> { self.inner.lock().unwrap().messages.clone() }

    /// Full session (v0 format) for the final transcript event.
    pub fn dump(&self) -> Value {
        let mut d = self.inner.lock().unwrap().dump();
        d["file"] = json!(self.path.to_string_lossy());
        d
    }
}

/// Extracts the text of a user message (string or text-block content).
pub fn extract_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
