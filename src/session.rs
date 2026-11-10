//! Session files in the UPSTREAM JSONL v3 format (WP-304): the same schema
//! pi's `SessionManager` writes, so `pi-rust` sessions open in pi and vice
//! versa.
//!
//! File layout: line 1 = header `{type:"session", version:3, id, timestamp,
//! cwd, parentSession?}`; then entries, each `{type, id, parentId,
//! timestamp}` with a per-type payload:
//! - `message` {message: AgentMessage}
//! - `session_info` {name?}
//! - `custom` {customType, data?} (extension `appendEntry`)
//! - `thinking_level_change` {thinkingLevel}
//! - `model_change` {provider, modelId}
//!
//! Entries form a tree via `id`/`parentId`; the current conversation is the
//! path root → leaf. v0 writes linear chains; `load` follows the leaf chain
//! (branching is handled structurally, forking a branch is T5/WP-305).
//!
//! Timestamps are ISO-8601 strings (upstream `new Date().toISOString()`).
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Session {
    pub path: PathBuf,
    pub id: String,
    pub cwd: String,
    pub parent_session: Option<String>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session({})", self.path.display())
    }
}

struct Inner {
    name: Option<String>,
    entries: Vec<Value>,
    messages: Vec<Value>, // AgentMessage transcript (root→leaf order)
    leaf_id: Option<String>,
    file: File,
}

impl Inner {
    fn write_line(file: &mut File, v: &Value) {
        let _ = writeln!(file, "{}", v.to_string());
        let _ = file.flush();
    }

    fn push(&mut self, entry: Value) {
        self.entries.push(entry.clone());
        self.leaf_id = entry.get("id").and_then(Value::as_str).map(str::to_string);
        Self::write_line(&mut self.file, &entry);
    }

    /// Appends an AgentMessage as a `message` entry (or `custom_message`
    /// for non-AgentMessage roles). Returns the entry id.
    fn append_message(&mut self, msg: &Value) -> String {
        let id = new_entry_id();
        let ts = now_iso();
        let role = msg.get("role").and_then(Value::as_str);
        let e = match role {
            Some("user" | "assistant" | "toolResult") => json!({"type":"message","id":id,"parentId":self.leaf_id,"timestamp":ts,"message":msg}),
            _ => json!({"type":"custom_message","id":id,"parentId":self.leaf_id,"timestamp":ts,
                "customType":"pi-rust","role":role.unwrap_or("unknown"),"message":msg}),
        };
        if e["type"] == "message" {
            self.messages.push(msg.clone());
        }
        self.push(e);
        id
    }

    /// Persists a sidecar `session_append_entry` payload (`{type:"custom",
    /// customType, data}`) as a `custom` entry.
    fn append_entry(&mut self, entry: Value) {
        let custom_type = entry.get("customType").cloned().unwrap_or(json!("pi-rust"));
        let data = entry.get("data").cloned().unwrap_or(Value::Null);
        self.push(json!({"type":"custom","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"customType":custom_type,"data":data}));
    }

    fn set_name(&mut self, name: Option<String>) {
        if name.as_deref() == self.name.as_deref() {
            return;
        }
        self.name = name.clone();
        self.push(json!({"type":"session_info","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"name":name}));
    }

    fn set_thinking_level(&mut self, level: &str) {
        self.push(json!({"type":"thinking_level_change","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"thinkingLevel":level}));
    }

    fn set_model(&mut self, provider: &str, model: &str) {
        self.push(json!({"type":"model_change","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"provider":provider,"modelId":model}));
    }
}

fn new_entry_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// ISO-8601 UTC with millisecond precision (upstream `toISOString()` shape).
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Millisecond epoch (bridge event timestamps; the session FILE uses ISO).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Upstream `getDefaultSessionDirPath` encoding: `--<cwd>` with the leading
/// `/` (or `\`) removed and every `/ \ :` replaced by `-`, wrapped in `--`.
pub fn encode_cwd(cwd: &str) -> String {
    let stripped = cwd.trim_start_matches(|c| c == '/' || c == '\\');
    let safe: String = stripped.chars().map(|c| match c {
        '/' | '\\' | ':' => '-',
        c => c,
    }).collect();
    format!("--{safe}--")
}

impl Session {
    /// Creates a NEW session file (writes the v3 header). Parent dirs are
    /// created.
    pub fn create(path: PathBuf, cwd: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let file = OpenOptions::new().create(true).append(true).open(&path)
            .with_context(|| format!("opening session file {}", path.display()))?;
        let header = json!({"type":"session","version":3,"id":id,"timestamp":now_iso(),"cwd":cwd});
        let inner = Inner { name: None, entries: Vec::new(), messages: Vec::new(), leaf_id: None, file };
        let mut s = Self { path, id: id.clone(), cwd: cwd.to_string(), parent_session: None, inner: Mutex::new(inner) };
        {
            let mut inner = s.inner.lock().unwrap();
            Inner::write_line(&mut inner.file, &header);
        }
        Ok(s)
    }

    /// Loads an existing session file (v3 header + entries), resuming at the
    /// leaf of the entry tree. v0-format files are rejected with a typed
    /// error (migration is a documented follow-up, not implicit).
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| format!("reading session file {}", path.display()))?;
        let mut lines = raw.lines();
        let first = lines.next().context("session file is empty")?;
        let header: Value = serde_json::from_str(first).with_context(|| "session header is not valid JSON")?;
        if header.get("type").and_then(Value::as_str) != Some("session") {
            bail!("{} is not a v3 session file (missing session header) — v0 files need migration", path.display());
        }
        let version = header.get("version").and_then(Value::as_u64).context("session header has no version")?;
        if version != 3 {
            bail!("unsupported session version {} in {}", version, path.display());
        }
        let id = header.get("id").and_then(Value::as_str).context("header has no id")?.to_string();
        let cwd = header.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
        let parent_session = header.get("parentSession").and_then(Value::as_str).map(str::to_string);

        let mut entries: Vec<Value> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let e: Value = serde_json::from_str(line).with_context(|| "session entry is not valid JSON")?;
            entries.push(e);
        }
        // Follow the leaf chain back to the root (handles branches; linear
        // files are the common case).
        let by_id: std::collections::HashMap<String, Value> = entries
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str).map(|i| (i.to_string(), e.clone())))
            .collect();
        let mut cur: Option<String> = entries.last().and_then(|e| e.get("id").and_then(Value::as_str)).map(str::to_string);
        let mut chain: Vec<Value> = Vec::new();
        while let Some(cid) = cur {
            let e = by_id.get(&cid).context("session entry references unknown parent")?.clone();
            let next = e.get("parentId").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string);
            chain.push(e);
            cur = next;
        }
        chain.reverse();
        let mut name: Option<String> = None;
        let mut messages: Vec<Value> = Vec::new();
        for e in &chain {
            match e.get("type").and_then(Value::as_str) {
                Some("session_info") => name = e.get("name").and_then(Value::as_str).map(str::to_string),
                Some("message") => {
                    if let Some(m) = e.get("message") {
                        messages.push(m.clone());
                    }
                }
                _ => {}
            }
        }

        let file = OpenOptions::new().append(true).open(path)
            .with_context(|| format!("reopening session file {}", path.display()))?;
        let leaf_id = chain.last().and_then(|e| e.get("id").and_then(Value::as_str)).map(str::to_string);
        let inner = Inner { name, entries, messages, leaf_id, file };
        Ok(Self { path: path.to_path_buf(), id, cwd, parent_session, inner: Mutex::new(inner) })
    }

    pub fn append_message(&self, msg: &Value) -> String {
        self.inner.lock().unwrap().append_message(msg)
    }

    /// Persists a sidecar `session_append_entry` payload as a `custom` entry.
    pub fn append_entry(&self, entry: Value) {
        self.inner.lock().unwrap().append_entry(entry);
    }

    pub fn set_name(&self, name: Option<String>) {
        self.inner.lock().unwrap().set_name(name);
    }

    pub fn set_thinking_level(&self, level: &str) {
        self.inner.lock().unwrap().set_thinking_level(level);
    }

    pub fn set_model(&self, provider: &str, model: &str) {
        self.inner.lock().unwrap().set_model(provider, model);
    }

    pub fn transcript(&self) -> Vec<Value> {
        self.inner.lock().unwrap().messages.clone()
    }

    /// Full session for the final transcript event (pi-rust event payload,
    /// not the file format).
    pub fn dump(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({"id":self.id,"cwd":self.cwd,"parentSession":self.parent_session,
            "name":inner.name,"entries":inner.entries,"messages":inner.messages,
            "file":self.path.to_string_lossy()})
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pi-rust-sess-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn user(text: &str) -> Value {
        json!({"role":"user","content":[{"type":"text","text":text}],"timestamp":"2025-01-01T00:00:00.000Z"})
    }
    fn assistant(text: &str) -> Value {
        json!({"role":"assistant","content":[{"type":"text","text":text}],"api":"fake","provider":"fake",
            "model":"fake","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},
            "stopReason":"stop","timestamp":"2025-01-01T00:00:01.000Z"})
    }

    #[test]
	fn creates_v3_header_and_entry_chain() {
        let d = tmpdir();
        let path = d.join("s.jsonl");
        let s = Session::create(path.clone(), "/work/demo").unwrap();
        let id1 = s.append_message(&user("hi"));
        let id2 = s.append_message(&assistant("hello"));
        s.set_name(Some("n".into()));
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        let h: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(h["type"], "session");
        assert_eq!(h["version"], 3);
        assert_eq!(h["cwd"], "/work/demo");
        assert_eq!(h["id"], s.id);
        let e1: Value = serde_json::from_str(lines[1]).unwrap();
        let e2: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(e1["type"], "message");
        assert!(e1["parentId"].is_null(), "first entry has no parent: {e1}");
        assert_eq!(e2["parentId"], id1, "chain: e2.parentId == e1.id");
        assert_eq!(e2["message"]["role"], "assistant");
        let e3: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(e3["type"], "session_info");
        assert_eq!(e3["name"], "n");
        assert!(lines[0].contains("timestamp"), "ISO timestamps present: {e1}");
    }

    #[test]
	fn round_trip_load_rebuilds_transcript_and_name() {
        let d = tmpdir();
        let path = d.join("rt.jsonl");
        {
            let s = Session::create(path.clone(), "/work/rt").unwrap();
            s.append_message(&user("a"));
            s.append_entry(json!({"customType":"my-ext","data":{"x":1}}));
            s.append_message(&assistant("b"));
            s.set_name(Some("named".into()));
        }
        let s = Session::load(&path).unwrap();
        assert_eq!(s.id, {
            let h: Value = serde_json::from_str(std::fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
            h["id"].as_str().unwrap().to_string()
        });
        assert_eq!(s.transcript().len(), 2);
        assert_eq!(s.transcript()[0]["role"], "user");
        assert_eq!(s.transcript()[1]["role"], "assistant");
        let dump = s.dump();
        assert_eq!(dump["entries"].as_array().unwrap().len(), 4);
        // Appending after resume continues the chain from the leaf.
        let new_id = s.append_message(&user("c"));
        let raw = std::fs::read_to_string(&path).unwrap();
        let last: Value = serde_json::from_str(raw.lines().last().unwrap()).unwrap();
        assert_eq!(last["id"], new_id);
        let prev: Value = serde_json::from_str(raw.lines().nth(4).unwrap()).unwrap();
        assert_eq!(last["parentId"], prev["id"]);
    }

    #[test]
	fn golden_upstream_shaped_file_loads() {
        // Hand-written in the documented upstream v3 shapes (incl. toolCall
        // with `input` and a custom entry), as pi would have written it.
        let d = tmpdir();
        let path = d.join("golden.jsonl");
        let content = "\
{\"type\":\"session\",\"version\":3,\"id\":\"ses-1\",\"timestamp\":\"2025-03-01T10:00:00.000Z\",\"cwd\":\"/home/user/proj\"}
{\"type\":\"message\",\"id\":\"e1\",\"parentId\":null,\"timestamp\":\"2025-03-01T10:00:01.000Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"use the tool\"}],\"timestamp\":\"2025-03-01T10:00:01.000Z\"}}
{\"type\":\"message\",\"id\":\"e2\",\"parentId\":\"e1\",\"timestamp\":\"2025-03-01T10:00:02.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"calling\"},{\"type\":\"toolCall\",\"id\":\"tc1\",\"name\":\"bash\",\"input\":{\"command\":\"ls\"}}],\"api\":\"openai-completions\",\"provider\":\"openai\",\"model\":\"gpt-4o\",\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"toolUse\",\"timestamp\":\"2025-03-01T10:00:02.000Z\"}}
{\"type\":\"message\",\"id\":\"e3\",\"parentId\":\"e2\",\"timestamp\":\"2025-03-01T10:00:03.000Z\",\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"tc1\",\"toolName\":\"bash\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"isError\":false,\"timestamp\":\"2025-03-01T10:00:03.000Z\"}}
{\"type\":\"custom\",\"id\":\"e4\",\"parentId\":\"e3\",\"timestamp\":\"2025-03-01T10:00:04.000Z\",\"customType\":\"my-ext\",\"data\":{\"k\":1}}
{\"type\":\"session_info\",\"id\":\"e5\",\"parentId\":\"e4\",\"timestamp\":\"2025-03-01T10:00:05.000Z\",\"name\":\"golden\"}
";
        std::fs::write(&path, content).unwrap();
        let s = Session::load(&path).unwrap();
        assert_eq!(s.id, "ses-1");
        assert_eq!(s.cwd, "/home/user/proj");
        assert_eq!(s.transcript().len(), 3, "user + assistant + toolResult");
        let a = &s.transcript()[1];
        assert_eq!(a["content"][1]["type"], "toolCall");
        assert_eq!(a["content"][1]["input"]["command"], "ls");
        let dump = s.dump();
        assert_eq!(dump["entries"].as_array().unwrap().len(), 5);
    }

    #[test]
	fn v0_file_rejected_with_typed_error() {
        let d = tmpdir();
        let path = d.join("v0.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"id\":\"e1\",\"timestamp\":1,\"text\":\"x\"}\n").unwrap();
        let err = Session::load(&path).unwrap_err().to_string();
        assert!(err.contains("migration") || err.contains("not a v3 session"), "{err}");
    }

    #[test]
	fn encode_cwd_matches_upstream() {
        assert_eq!(encode_cwd("/home/user/proj"), "--home-user-proj--");
        assert_eq!(encode_cwd("relative/x"), "--relative-x--");
    }
}
