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
    /// WP-609: identity fields are lock-protected — the TUI's runtime
    /// resume (`resume_from`) swaps them in place.
    pub path: Mutex<PathBuf>,
    pub id: Mutex<String>,
    pub cwd: Mutex<String>,
    pub parent_session: Mutex<Option<String>>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session({})", self.path.lock().unwrap().display())
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
        let _ = writeln!(file, "{}", v);
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
            Some("user" | "assistant" | "toolResult") => {
                json!({"type":"message","id":id,"parentId":self.leaf_id,"timestamp":ts,"message":msg})
            }
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
        self.push(
            json!({"type":"custom","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"customType":custom_type,"data":data}),
        );
    }

    fn set_name(&mut self, name: Option<String>) {
        if name.as_deref() == self.name.as_deref() {
            return;
        }
        self.name = name.clone();
        self.push(
            json!({"type":"session_info","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"name":name}),
        );
    }

    fn set_thinking_level(&mut self, level: &str) {
        self.push(
            json!({"type":"thinking_level_change","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"thinkingLevel":level}),
        );
    }

    fn set_model(&mut self, provider: &str, model: &str) {
        self.push(
            json!({"type":"model_change","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),"provider":provider,"modelId":model}),
        );
    }

    fn set_usage(&mut self, usage: &crate::model::Usage, cost_usd: f64) {
        self.push(
            json!({"type":"usage","id":new_entry_id(),"parentId":self.leaf_id,
            "timestamp":now_iso(),
            "usage":{"input":usage.input,"output":usage.output,
                     "cacheRead":usage.cache_read,"cacheWrite":usage.cache_write},
            "costUsd":cost_usd}),
        );
    }

    /// WP-704: append a compaction entry (upstream CompactionEntry shape)
    /// and re-project the transcript (upstream: the session is reloaded after
    /// compaction). `fromHook` is written only for extension-supplied
    /// summaries; `usage`/`systemMessage` are included when available.
    fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        usage: Option<&crate::model::Usage>,
        from_hook: bool,
        system_message: Option<&Value>,
    ) -> String {
        let id = new_entry_id();
        let mut e = json!({"type":"compaction","id":id,"parentId":self.leaf_id,"timestamp":now_iso(),
            "summary":summary,"firstKeptEntryId":first_kept_entry_id,"tokensBefore":tokens_before});
        if let Some(u) = usage {
            e["usage"] = json!({"input":u.input,"output":u.output,"cacheRead":u.cache_read,"cacheWrite":u.cache_write});
        }
        if from_hook {
            e["fromHook"] = json!(true);
        }
        if let Some(s) = system_message {
            e["systemMessage"] = s.clone();
        }
        self.push(e);
        self.messages = project_messages(&self.entries);
        id
    }
}

fn new_entry_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// ISO-8601 UTC with millisecond precision (upstream `toISOString()` shape).
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// WP-802: epoch-milliseconds timestamp (the upstream wire form).
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ISO-8601 UTC from unix seconds (list-sessions `updated`).
pub fn iso_from_secs(secs: u64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
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
    let stripped = cwd.trim_start_matches(['/', '\\']);
    let safe: String = stripped
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c => c,
        })
        .collect();
    format!("--{safe}--")
}

// ============================================================================
// WP-704: context compaction — port of upstream core/compaction (pin 085c84e):
// threshold, token estimate, cut point, summarization prompts/serialization,
// and the compaction-aware transcript projection (buildContextEntries).
// ============================================================================

/// Upstream DEFAULT_COMPACTION_SETTINGS (compaction.ts).
pub const DEFAULT_RESERVE_TOKENS: u64 = 16384;
pub const DEFAULT_KEEP_RECENT_TOKENS: u64 = 20000;

const ESTIMATED_IMAGE_CHARS: u64 = 4800;
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Verbatim port of upstream core/compaction/utils.ts:162 (pin 085c84e).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r##"You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."##;

/// Verbatim port of upstream core/compaction/compaction.ts:507 (pin 085c84e).
pub const SUMMARIZATION_PROMPT: &str = r##"The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or "(none)" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages."##;

/// Verbatim port of upstream core/compaction/compaction.ts:577 (pin 085c84e).
pub const UPDATE_SUMMARIZATION_PROMPT: &str = r##"The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from "In Progress" to "Done" when completed
- UPDATE "Next Steps" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages."##;

/// Upstream estimateTextAndImageContentChars: string or text blocks count
/// their chars; an image block counts as ESTIMATED_IMAGE_CHARS.
fn content_chars(content: &Value) -> u64 {
    match content {
        Value::String(s) => s.chars().count() as u64,
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().count() as u64)
                    .unwrap_or(0),
                Some("image") => ESTIMATED_IMAGE_CHARS,
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

/// Upstream estimateTokens: the conservative chars/4 heuristic per message.
pub fn estimate_tokens(msg: &Value) -> u64 {
    let chars = match msg.get("role").and_then(Value::as_str).unwrap_or("") {
        "system" => {
            let mut chars = content_chars(msg.get("content").unwrap_or(&Value::Null));
            if let Some(tools) = msg.get("toolsAdded") {
                chars += tools.to_string().chars().count() as u64;
            }
            chars
        }
        "user" | "toolResult" | "custom" | "compactionSummary" => {
            content_chars(msg.get("content").unwrap_or(&Value::Null))
        }
        "assistant" => {
            let mut chars = 0u64;
            if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str).unwrap_or("") {
                        "text" => {
                            chars += b
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|t| t.chars().count() as u64)
                                .unwrap_or(0)
                        }
                        "thinking" => {
                            chars += b
                                .get("thinking")
                                .and_then(Value::as_str)
                                .map(|t| t.chars().count() as u64)
                                .unwrap_or(0)
                        }
                        "toolCall" => {
                            chars += b
                                .get("name")
                                .and_then(Value::as_str)
                                .map(|n| n.chars().count() as u64)
                                .unwrap_or(0);
                            // pi-rust session files carry the arguments as `input`
                            // (upstream LLM shape: `arguments`).
                            if let Some(args) = b.get("arguments").or_else(|| b.get("input")) {
                                chars += args.to_string().chars().count() as u64;
                            }
                        }
                        _ => {}
                    }
                }
            }
            chars
        }
        _ => 0,
    };
    chars.div_ceil(4)
}

/// Upstream calculateContextTokens for one assistant message's usage;
/// aborted/error and all-zero usages are skipped (no valid usage data).
fn assistant_usage_tokens(m: &Value) -> Option<u64> {
    if m.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    if m.get("stopReason")
        .and_then(Value::as_str)
        .is_some_and(|s| s == "aborted" || s == "error")
    {
        return None;
    }
    let u = m.get("usage")?;
    let tokens = u
        .get("totalTokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            u.get("input").and_then(Value::as_u64).unwrap_or(0)
                + u.get("output").and_then(Value::as_u64).unwrap_or(0)
                + u.get("cacheRead").and_then(Value::as_u64).unwrap_or(0)
                + u.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0)
        });
    (tokens > 0).then_some(tokens)
}

/// Upstream estimateContextTokens: the last valid assistant usage plus the
/// estimate of every message after it (all messages when there is no usage).
pub fn estimate_context_tokens(messages: &[Value]) -> u64 {
    let mut last_usage: Option<(usize, u64)> = None;
    for (i, m) in messages.iter().enumerate() {
        if let Some(u) = assistant_usage_tokens(m) {
            last_usage = Some((i, u));
        }
    }
    match last_usage {
        Some((i, usage_tokens)) => {
            usage_tokens + messages[i + 1..].iter().map(estimate_tokens).sum::<u64>()
        }
        None => messages.iter().map(estimate_tokens).sum(),
    }
}

/// Upstream shouldCompact: contextTokens > contextWindow - reserveTokens.
pub fn should_compact(context_tokens: u64, context_window: u64, reserve_tokens: u64) -> bool {
    // Upstream: `contextTokens > contextWindow - reserveTokens` (JS numbers:
    // a negative threshold means "always compact"); mirror with i64.
    context_tokens as i64 > context_window as i64 - reserve_tokens as i64
}

/// The AgentMessage of a `message` entry (None for non-message entries).
fn entry_message(entry: &Value) -> Option<&Value> {
    if entry.get("type").and_then(Value::as_str) == Some("message") {
        entry.get("message")
    } else {
        None
    }
}

/// Upstream isCutPointMessage: user-like or assistant messages are cut
/// points; toolResult never is (it must follow its tool call). pi-rust v0
/// stores only user/assistant/toolResult messages.
fn is_cut_point(msg: &Value) -> bool {
    matches!(
        msg.get("role").and_then(Value::as_str),
        Some("user") | Some("assistant")
    )
}

/// Upstream isTurnStartMessage for pi-rust roles: a user message.
fn is_turn_start(msg: &Value) -> bool {
    msg.get("role").and_then(Value::as_str) == Some("user")
}

/// Upstream CutPointResult (findCutPoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPoint {
    /// Index of the first entry to keep.
    pub first_kept_index: usize,
    /// Index of the user message that starts the turn being split (None when
    /// not splitting).
    pub turn_start_index: Option<usize>,
    /// Whether the cut splits a turn (cut point is not a user message).
    pub is_split_turn: bool,
}

/// Upstream findCutPoint: walk backwards from the newest, accumulating
/// estimated message sizes until keep_recent_tokens is met; cut at the
/// closest valid cut point at or after that entry (never a tool result; an
/// assistant with tool calls keeps its results). Adjacent context-invisible
/// metadata entries are included in the kept range.
pub fn find_cut_point(
    entries: &[Value],
    start: usize,
    end: usize,
    keep_recent_tokens: u64,
) -> CutPoint {
    let cut_points: Vec<usize> = (start..end)
        .filter(|&i| entry_message(entries.get(i).unwrap()).is_some_and(is_cut_point))
        .collect();
    if cut_points.is_empty() {
        return CutPoint {
            first_kept_index: start,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated = 0u64;
    let mut cut_index = cut_points[0];
    for i in (start..end).rev() {
        let tokens = entry_message(&entries[i]).map(estimate_tokens).unwrap_or(0);
        if tokens == 0 {
            continue;
        }
        accumulated += tokens;
        if accumulated >= keep_recent_tokens {
            cut_index = cut_points
                .iter()
                .copied()
                .find(|&c| c >= i)
                .unwrap_or(*cut_points.last().unwrap());
            break;
        }
    }

    while cut_index > start {
        let prev = &entries[cut_index - 1];
        if prev.get("type").and_then(Value::as_str) == Some("compaction")
            || entry_message(prev).is_some()
        {
            break;
        }
        cut_index -= 1;
    }

    let starts_turn = entry_message(&entries[cut_index]).is_some_and(is_turn_start);
    let turn_start_index = if starts_turn {
        None
    } else {
        (start..=cut_index)
            .rev()
            .find(|&i| entry_message(&entries[i]).is_some_and(is_turn_start))
    };
    CutPoint {
        first_kept_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index.is_some(),
    }
}

/// Upstream CompactionPreparation reduced to what the v0 core needs (the
/// split-turn TURN_PREFIX summary is a documented v0 non-goal).
#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    /// Id of the first entry to keep.
    pub first_kept_entry_id: String,
    /// The AgentMessage JSON about to be summarized and discarded.
    pub messages_to_summarize: Vec<Value>,
    pub tokens_before: u64,
    /// Summary from the previous compaction entry (iterative update).
    pub previous_summary: Option<String>,
}

/// Upstream prepareCompaction over the session entry path. The cut domain is
/// the previous compaction's kept tail plus everything after that compaction;
/// a split-turn cut snaps back to the turn start (v0: no TURN_PREFIX summary,
/// so a turn is never half-kept). None when the session ends at a compaction
/// or there is nothing to summarize.
pub fn prepare_compaction(
    entries: &[Value],
    system_message: &Value,
    keep_recent_tokens: u64,
) -> Option<CompactionPreparation> {
    let last = entries.last()?;
    if last.get("type").and_then(Value::as_str) == Some("compaction") {
        return None; // already compacted (upstream: "Already compacted")
    }
    let compaction_idx = entries
        .iter()
        .rposition(|e| e.get("type").and_then(Value::as_str) == Some("compaction"));
    let (domain_start, previous_summary) = match compaction_idx {
        Some(ci) => {
            let first_kept = entries[ci]
                .get("firstKeptEntryId")
                .and_then(Value::as_str)
                .unwrap_or("");
            let start = entries[..ci]
                .iter()
                .position(|e| e.get("id").and_then(Value::as_str) == Some(first_kept))
                .unwrap_or(0);
            (
                start,
                Some(
                    entries[ci]
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
            )
        }
        None => (0, None),
    };
    let domain: Vec<Value> = match compaction_idx {
        Some(ci) => entries[domain_start..ci]
            .iter()
            .chain(entries[ci + 1..].iter())
            .cloned()
            .collect(),
        None => entries.to_vec(),
    };
    // Upstream estimateProjectedContextTokens over the projected context:
    // the system message once, plus the projected transcript.
    let mut convo = vec![system_message.clone()];
    convo.extend(project_messages(entries));
    let tokens_before = estimate_context_tokens(&convo);

    let cut = find_cut_point(&domain, 0, domain.len(), keep_recent_tokens);
    let first_kept = if cut.is_split_turn {
        // v0 non-goal: the split-turn TURN_PREFIX summary — keep the whole turn.
        cut.turn_start_index
            .expect("is_split_turn requires a turn start")
    } else {
        cut.first_kept_index
    };
    let first_kept_entry_id = domain[first_kept]
        .get("id")
        .and_then(Value::as_str)?
        .to_string();
    let messages_to_summarize: Vec<Value> = domain[..first_kept]
        .iter()
        .filter_map(|e| entry_message(e).cloned())
        .filter(|m| m.get("role").and_then(Value::as_str) != Some("system"))
        .collect();
    if messages_to_summarize.is_empty() {
        return None; // nothing to compact (session too small)
    }
    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        tokens_before,
        previous_summary,
    })
}

/// Upstream contentText: text blocks joined with `separator`.
fn content_text(content: &Value, separator: &str) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(separator),
        _ => String::new(),
    }
}

/// Upstream truncateForSummary: head + "[... N more characters truncated]".
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!(
        "{head}\n\n[... {} more characters truncated]",
        total - max_chars
    )
}

/// Upstream serializeConversation: [User] / [Assistant] / [Assistant tool
/// calls] / [Tool result] lines; tool results truncated to 2000 chars;
/// thinking omitted. shortcut: tool-call argument keys serialize sorted
/// (serde_json BTreeMap), not in insertion order — cosmetic for multi-arg tools.
pub fn serialize_conversation(messages: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in messages {
        match m.get("role").and_then(Value::as_str).unwrap_or("") {
            "user" => {
                let text = content_text(m.get("content").unwrap_or(&Value::Null), "");
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            "assistant" => {
                let blocks = m
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut tool_calls: Vec<String> = Vec::new();
                for b in &blocks {
                    if b.get("type").and_then(Value::as_str) == Some("toolCall") {
                        let name = b.get("name").and_then(Value::as_str).unwrap_or_default();
                        let args = b
                            .get("arguments")
                            .cloned()
                            .or_else(|| b.get("input").cloned())
                            .unwrap_or_else(|| json!({}));
                        let args_str = match args {
                            Value::Object(map) => map
                                .iter()
                                .map(|(k, v)| format!("{k}={}", v))
                                .collect::<Vec<_>>()
                                .join(", "),
                            other => other.to_string(),
                        };
                        tool_calls.push(format!("{name}({args_str})"));
                    }
                }
                if blocks
                    .iter()
                    .any(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                {
                    parts.push(format!(
                        "[Assistant]: {}",
                        content_text(m.get("content").unwrap_or(&Value::Null), "\n")
                    ));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            "toolResult" => {
                let text = content_text(m.get("content").unwrap_or(&Value::Null), "");
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}

/// The upstream buildContextEntries projection as messages: the newest
/// compaction entry becomes one compactionSummary message, followed by the
/// messages of the kept range (firstKeptEntryId .. compaction, inclusive of
/// the first kept entry) and all entries after the compaction. Entries before
/// the kept range are summarized away. An older compaction entry inside the
/// kept range contributes nothing (upstream: only the newest at index 0
/// contributes its checkpoint). Applies to fresh runs AND resume-from-file.
pub fn project_messages(entries: &[Value]) -> Vec<Value> {
    let Some(compaction_idx) = entries
        .iter()
        .rposition(|e| e.get("type").and_then(Value::as_str) == Some("compaction"))
    else {
        return entries
            .iter()
            .filter_map(|e| entry_message(e).cloned())
            .collect();
    };
    let compaction = &entries[compaction_idx];
    let first_kept = compaction
        .get("firstKeptEntryId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut out: Vec<Value> = vec![json!({
        "role": "compactionSummary",
        "content": [{"type": "text", "text": compaction.get("summary").and_then(Value::as_str).unwrap_or_default()}],
        "timestamp": compaction.get("timestamp").cloned().unwrap_or(Value::Null),
    })];
    let mut found_first_kept = false;
    for e in &entries[..compaction_idx] {
        if e.get("id").and_then(Value::as_str) == Some(first_kept) {
            found_first_kept = true;
        }
        if found_first_kept {
            if let Some(m) = entry_message(e) {
                out.push(m.clone());
            }
        }
    }
    for e in &entries[compaction_idx + 1..] {
        if let Some(m) = entry_message(e) {
            out.push(m.clone());
        }
    }
    out
}

impl Session {
    /// Creates a NEW session file (writes the v3 header). Parent dirs are
    /// created.
    pub fn create(path: PathBuf, cwd: &str) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening session file {}", path.display()))?;
        let header = json!({"type":"session","version":3,"id":id,"timestamp":now_iso(),"cwd":cwd});
        let inner = Inner {
            name: None,
            entries: Vec::new(),
            messages: Vec::new(),
            leaf_id: None,
            file,
        };
        let s = Self {
            path: Mutex::new(path),
            id: Mutex::new(id.clone()),
            cwd: Mutex::new(cwd.to_string()),
            parent_session: Mutex::new(None),
            inner: Mutex::new(inner),
        };
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
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading session file {}", path.display()))?;
        let mut lines = raw.lines();
        let first = lines.next().context("session file is empty")?;
        let header: Value =
            serde_json::from_str(first).with_context(|| "session header is not valid JSON")?;
        if header.get("type").and_then(Value::as_str) != Some("session") {
            bail!(
                "{} is not a v3 session file (missing session header) — v0 files need migration",
                path.display()
            );
        }
        let version = header
            .get("version")
            .and_then(Value::as_u64)
            .context("session header has no version")?;
        if version != 3 {
            bail!(
                "unsupported session version {} in {}",
                version,
                path.display()
            );
        }
        let id = header
            .get("id")
            .and_then(Value::as_str)
            .context("header has no id")?
            .to_string();
        let cwd = header
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let parent_session = header
            .get("parentSession")
            .and_then(Value::as_str)
            .map(str::to_string);

        let mut entries: Vec<Value> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let e: Value =
                serde_json::from_str(line).with_context(|| "session entry is not valid JSON")?;
            entries.push(e);
        }
        // Follow the leaf chain back to the root (handles branches; linear
        // files are the common case).
        let by_id: std::collections::HashMap<String, Value> = entries
            .iter()
            .filter_map(|e| {
                e.get("id")
                    .and_then(Value::as_str)
                    .map(|i| (i.to_string(), e.clone()))
            })
            .collect();
        let mut cur: Option<String> = entries
            .last()
            .and_then(|e| e.get("id").and_then(Value::as_str))
            .map(str::to_string);
        let mut chain: Vec<Value> = Vec::new();
        while let Some(cid) = cur {
            let e = by_id
                .get(&cid)
                .context("session entry references unknown parent")?
                .clone();
            let next = e
                .get("parentId")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .map(str::to_string);
            chain.push(e);
            cur = next;
        }
        chain.reverse();
        let mut name: Option<String> = None;
        for e in &chain {
            if e.get("type").and_then(Value::as_str) == Some("session_info") {
                name = e.get("name").and_then(Value::as_str).map(str::to_string);
            }
        }
        // WP-704: the transcript is the compaction-aware projection (a pi
        // file with compaction entries loads the way upstream
        // buildContextEntries projects it).
        let messages = project_messages(&chain);

        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("reopening session file {}", path.display()))?;
        let leaf_id = chain
            .last()
            .and_then(|e| e.get("id").and_then(Value::as_str))
            .map(str::to_string);
        let inner = Inner {
            name,
            entries,
            messages,
            leaf_id,
            file,
        };
        Ok(Self {
            path: Mutex::new(path.to_path_buf()),
            id: Mutex::new(id),
            cwd: Mutex::new(cwd),
            parent_session: Mutex::new(parent_session),
            inner: Mutex::new(inner),
        })
    }

    /// Creates a fork of an existing session: a NEW file (new id) whose
    /// header carries `parentSession` = the original id, with all original
    /// entries copied. The original file is not touched.
    /// WP-609: replace this session's identity and file with the one at
    /// `path` (runtime resume from the TUI session picker). The previous
    /// file is left untouched; the in-memory name/entries of the loaded
    /// file take over.
    pub fn resume_from(&self, path: &Path) -> Result<()> {
        let next = Self::load(path)?;
        let mut inner = self.inner.lock().unwrap();
        let mut next_inner = next.inner.lock().unwrap();
        inner.name = next_inner.name.clone();
        inner.entries = std::mem::take(&mut next_inner.entries);
        inner.messages = std::mem::take(&mut next_inner.messages);
        inner.leaf_id = next_inner.leaf_id.clone();
        inner.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(next.path.lock().unwrap().clone())
            .with_context(|| format!("opening session file {}", path.display()))?;
        drop(inner);
        *self.path.lock().unwrap() = next.path.lock().unwrap().clone();
        *self.id.lock().unwrap() = next.id.lock().unwrap().clone();
        *self.cwd.lock().unwrap() = next.cwd.lock().unwrap().clone();
        *self.parent_session.lock().unwrap() = next.parent_session.lock().unwrap().clone();
        Ok(())
    }

    pub fn fork(old_path: &Path, new_path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(old_path)
            .with_context(|| format!("reading session file {}", old_path.display()))?;
        let mut lines = raw.lines();
        let first = lines.next().context("session file is empty")?;
        let header: Value =
            serde_json::from_str(first).context("fork source has no JSON header")?;
        if header.get("type").and_then(Value::as_str) != Some("session") {
            bail!(
                "{} is not a v3 session file — cannot fork",
                old_path.display()
            );
        }
        let old_id = header
            .get("id")
            .and_then(Value::as_str)
            .context("fork source has no id")?;
        let cwd = header
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(new_path)
            .with_context(|| format!("opening fork target {}", new_path.display()))?;
        let new_id = uuid::Uuid::new_v4().to_string();
        let h = json!({"type":"session","version":3,"id":new_id,"timestamp":now_iso(),
            "cwd":cwd,"parentSession":old_id});
        writeln!(file, "{}", h).context("writing fork header")?;
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            writeln!(file, "{line}").context("writing fork entries")?;
        }
        file.flush().ok();
        drop(file);
        Self::load(new_path)
    }

    pub fn append_message(&self, msg: &Value) -> String {
        self.inner.lock().unwrap().append_message(msg)
    }

    /// WP-610: the first JSON line of the session file (the header) —
    /// leading blank lines are skipped (upstream json mode writes it first).
    pub fn header(&self) -> Option<Value> {
        let path = self.path.lock().unwrap().clone();
        let raw = std::fs::read_to_string(&path).ok()?;
        for line in raw.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            return serde_json::from_str(t).ok();
        }
        None
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

    pub fn set_usage(&self, usage: &crate::model::Usage, cost_usd: f64) {
        self.inner.lock().unwrap().set_usage(usage, cost_usd);
    }

    /// WP-704: persist a compaction (upstream CompactionEntry shape) and
    /// re-project the transcript. Returns the entry id.
    pub fn append_compaction(
        &self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        usage: Option<&crate::model::Usage>,
        from_hook: bool,
        system_message: Option<&Value>,
    ) -> String {
        self.inner.lock().unwrap().append_compaction(
            summary,
            first_kept_entry_id,
            tokens_before,
            usage,
            from_hook,
            system_message,
        )
    }

    /// WP-704: the session entry path (root→leaf order) — compaction
    /// preparation and the projection work on this slice.
    pub fn entries(&self) -> Vec<Value> {
        self.inner.lock().unwrap().entries.clone()
    }

    pub fn transcript(&self) -> Vec<Value> {
        self.inner.lock().unwrap().messages.clone()
    }

    /// Full session for the final transcript event (pi-rust event payload,
    /// not the file format).
    pub fn dump(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        json!({"id":self.id.lock().unwrap().clone(),"cwd":self.cwd.lock().unwrap().clone(),"parentSession":self.parent_session.lock().unwrap().clone(),
            "name":inner.name,"entries":inner.entries,"messages":inner.messages,
            "file":self.path.lock().unwrap().to_string_lossy()})
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

/// One session file under `<agentDir>/sessions/` (the shared source for
/// `--list-sessions` and the TUI session picker, WP-609).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub path: PathBuf,
    pub id: String,
    pub name: Option<String>,
    pub cwd: String,
    pub parent_session: Option<String>,
    pub updated: Option<String>,
    pub entries: usize,
}

/// Lists session files under `<agentDir>/sessions/` (recursive), most
/// recently modified first. Unreadable/unparseable files are skipped.
pub fn list(agent_dir: &Path) -> Vec<SessionInfo> {
    let root = agent_dir.join("sessions");
    let mut files = Vec::new();
    if root.exists() {
        collect_session_files(&root, &mut files);
    }
    let mut out = Vec::new();
    for f in &files {
        let Ok(raw) = std::fs::read_to_string(f) else {
            continue;
        };
        let Some(first) = raw.lines().next() else {
            continue;
        };
        let Ok(h) = serde_json::from_str::<Value>(first) else {
            continue;
        };
        if h.get("type").and_then(Value::as_str) != Some("session") {
            continue;
        }
        let id = h
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let cwd = h
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let parent_session = h
            .get("parentSession")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut name: Option<String> = None;
        let mut entries = 0usize;
        for line in raw.lines().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            entries += 1;
            if let Ok(e) = serde_json::from_str::<Value>(line) {
                if e.get("type").and_then(Value::as_str) == Some("session_info") {
                    name = e.get("name").and_then(Value::as_str).map(str::to_string);
                }
            }
        }
        let updated = std::fs::metadata(f)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| iso_from_secs(d.as_secs()));
        out.push(SessionInfo {
            path: f.clone(),
            id,
            name,
            cwd,
            parent_session,
            updated,
            entries,
        });
    }
    out.sort_by(|a, b| b.path.cmp(&a.path)); // most recently modified first (path-embedded timestamp, as before)
    out
}

fn collect_session_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_session_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WP-609: resume_from swaps identity + file in place; appends after
    /// the resume land in the NEW file, the old file stays untouched.
    #[test]
    fn resume_from_swaps_file_and_identity() {
        let dir = std::env::temp_dir().join(format!("pi-rust-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.jsonl");
        let b = dir.join("b.jsonl");
        let s = Session::create(a.clone(), "/w").unwrap();
        let other = Session::create(b.clone(), "/other").unwrap();
        other.append_message(&json!({"role":"user","content":"from-b","timestamp":"t"}));
        let old_id = s.id.lock().unwrap().clone();
        let new_id = other.id.lock().unwrap().clone();
        s.resume_from(&b).unwrap();
        assert_eq!(s.id.lock().unwrap().as_str(), new_id);
        assert_ne!(s.id.lock().unwrap().as_str(), old_id);
        assert_eq!(s.cwd.lock().unwrap().as_str(), "/other");
        // The resumed transcript is visible through the same handle.
        let t = s.transcript();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0]["content"], "from-b");
        // Appends now land in file b.
        s.append_message(&json!({"role":"user","content":"after","timestamp":"t2"}));
        let b_lines = std::fs::read_to_string(&b).unwrap().lines().count();
        let a_lines = std::fs::read_to_string(&a).unwrap().lines().count();
        assert_eq!(a_lines, 1, "old file untouched (header only)");
        assert!(b_lines >= 3, "new file gets the append: {b_lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-sess-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
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
        let _id2 = s.append_message(&assistant("hello"));
        s.set_name(Some("n".into()));
        let raw = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        let h: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(h["type"], "session");
        assert_eq!(h["version"], 3);
        assert_eq!(h["cwd"], "/work/demo");
        assert_eq!(h["id"], s.id.lock().unwrap().clone());
        let e1: Value = serde_json::from_str(lines[1]).unwrap();
        let e2: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(e1["type"], "message");
        assert!(e1["parentId"].is_null(), "first entry has no parent: {e1}");
        assert_eq!(e2["parentId"], id1, "chain: e2.parentId == e1.id");
        assert_eq!(e2["message"]["role"], "assistant");
        let e3: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(e3["type"], "session_info");
        assert_eq!(e3["name"], "n");
        assert!(
            lines[0].contains("timestamp"),
            "ISO timestamps present: {e1}"
        );
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
        assert_eq!(s.id.lock().unwrap().clone(), {
            let h: Value = serde_json::from_str(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap(),
            )
            .unwrap();
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
        assert_eq!(s.id.lock().unwrap().as_str(), "ses-1");
        assert_eq!(s.cwd.lock().unwrap().as_str(), "/home/user/proj");
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
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"id\":\"e1\",\"timestamp\":1,\"text\":\"x\"}\n",
        )
        .unwrap();
        let err = Session::load(&path).unwrap_err().to_string();
        assert!(
            err.contains("migration") || err.contains("not a v3 session"),
            "{err}"
        );
    }

    #[test]
    fn encode_cwd_matches_upstream() {
        assert_eq!(encode_cwd("/home/user/proj"), "--home-user-proj--");
        assert_eq!(encode_cwd("relative/x"), "--relative-x--");
    }
}

#[cfg(test)]
mod wp704_tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-wp704-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn msg_entry(id: &str, parent: Option<&str>, role: &str, text: &str) -> Value {
        json!({"type":"message","id":id,"parentId":parent,"timestamp":"t","message":{
            "role":role,"content":[{"type":"text","text":text}],"timestamp":"t"}})
    }

    #[test]
    fn estimate_tokens_uses_chars_div_4() {
        assert_eq!(
            estimate_tokens(&json!({"role":"user","content":[{"type":"text","text":"abcd"}]})),
            1
        );
        assert_eq!(
            estimate_tokens(&json!({"role":"user","content":[{"type":"text","text":"abcde"}]})),
            2,
            "ceil(chars/4)"
        );
        // An image block counts as 4800 chars (1200 tokens).
        assert_eq!(
            estimate_tokens(
                &json!({"role":"user","content":[{"type":"image","mimeType":"image/png","data":"x"}]})
            ),
            1200
        );
        // assistant: text + thinking + toolCall (name + args JSON).
        let a = json!({"role":"assistant","content":[
            {"type":"text","text":"ab"},
            {"type":"thinking","thinking":"cd"},
            {"type":"toolCall","id":"t","name":"bash","input":{"command":"ls"}}
        ]});
        assert_eq!(estimate_tokens(&a), 6, "2+2+4+15=23 -> 6");
        // system: content + toolsAdded JSON length.
        let s = json!({"role":"system","content":"abcd","toolsAdded":[{"name":"bash"}]});
        assert_eq!(estimate_tokens(&s), 6, "4+17=21 -> 6");
        // compactionSummary counts the summary text; unknown roles are 0.
        assert_eq!(
            estimate_tokens(
                &json!({"role":"compactionSummary","content":[{"type":"text","text":"abcd"}]})
            ),
            1
        );
        assert_eq!(estimate_tokens(&json!({"role":"weird"})), 0);
    }

    #[test]
    fn estimate_context_tokens_uses_last_valid_assistant_usage() {
        let user = || json!({"role":"user","content":[{"type":"text","text":"abcd"}]}); // 1 token
        let assistant = |usage: Value, stop: &str| json!({"role":"assistant","content":[{"type":"text","text":"abcd"}],"usage":usage,"stopReason":stop});
        let zero = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0});
        // No (or zero) usage: plain sum of estimates.
        assert_eq!(
            estimate_context_tokens(&[user(), assistant(zero.clone(), "stop")]),
            2
        );
        // Valid usage: usage tokens + the estimate of the trailing message.
        assert_eq!(
            estimate_context_tokens(&[
                user(),
                assistant(
                    json!({"input":10,"output":2,"cacheRead":0,"cacheWrite":0}),
                    "stop"
                ),
                user()
            ]),
            13
        );
        // totalTokens wins; error/aborted usages are invalid.
        assert_eq!(
            estimate_context_tokens(&[assistant(json!({"totalTokens":7}), "stop"), user()]),
            8
        );
        assert_eq!(
            estimate_context_tokens(&[
                assistant(
                    json!({"input":50,"output":0,"cacheRead":0,"cacheWrite":0}),
                    "error"
                ),
                user()
            ]),
            2
        );
        assert_eq!(
            estimate_context_tokens(&[
                assistant(
                    json!({"input":50,"output":0,"cacheRead":0,"cacheWrite":0}),
                    "aborted"
                ),
                user()
            ]),
            2
        );
    }

    #[test]
    fn should_compact_threshold_boundary() {
        assert!(should_compact(1101, 1500, 400));
        assert!(!should_compact(1100, 1500, 400), "strictly greater");
        assert!(
            should_compact(0, 100, 300),
            "reserve > window saturates to 0"
        );
    }

    #[test]
    fn find_cut_point_semantics() {
        let u1 = msg_entry("u1", None, "user", "u1");
        let a1 = json!({"type":"message","id":"a1","parentId":"u1","timestamp":"t","message":{
            "role":"assistant","content":[{"type":"toolCall","id":"t1","name":"bash","input":{"command":"echo hi"}}],"timestamp":"t"}});
        let tr1 = json!({"type":"message","id":"tr1","parentId":"a1","timestamp":"t","message":{
            "role":"toolResult","toolCallId":"t1","toolName":"bash","content":[{"type":"text","text":"x".repeat(1000)}],"isError":false,"timestamp":"t"}});
        let u2 = msg_entry("u2", Some("tr1"), "user", "u2");
        let a2 = msg_entry("a2", Some("u2"), "assistant", "a2");
        let entries = vec![u1.clone(), a1.clone(), tr1.clone(), u2.clone(), a2.clone()];

        // keep=0: the walk lands on a2 (assistant) — a split turn at u2.
        let cut = find_cut_point(&entries, 0, 5, 0);
        assert_eq!(
            cut,
            CutPoint {
                first_kept_index: 4,
                turn_start_index: Some(3),
                is_split_turn: true
            }
        );

        // A budget met at the tool result keeps the NEXT cut point (u2); the
        // tool pair before it is summarized together, never split.
        let cut = find_cut_point(&entries, 0, 5, 250);
        assert_eq!(
            cut,
            CutPoint {
                first_kept_index: 3,
                turn_start_index: None,
                is_split_turn: false
            }
        );

        // A big budget keeps from the first cut point.
        assert_eq!(
            find_cut_point(&entries, 0, 5, 10_000),
            CutPoint {
                first_kept_index: 0,
                turn_start_index: None,
                is_split_turn: false
            }
        );

        // An assistant with tool calls keeps its result (no user before it:
        // not a split turn, the result is kept with the call).
        let pair = vec![a1.clone(), tr1.clone()];
        assert_eq!(
            find_cut_point(&pair, 0, 2, 0),
            CutPoint {
                first_kept_index: 0,
                turn_start_index: None,
                is_split_turn: false
            }
        );

        // No cut points (only a tool result): keep from the start.
        assert_eq!(
            find_cut_point(std::slice::from_ref(&tr1), 0, 1, 0),
            CutPoint {
                first_kept_index: 0,
                turn_start_index: None,
                is_split_turn: false
            }
        );

        // Adjacent context-invisible metadata entries are included in the kept range.
        let custom = json!({"type":"custom","id":"c1","parentId":"u1","timestamp":"t","customType":"x","data":{}});
        let entries = vec![u1.clone(), custom, u2.clone()];
        assert_eq!(
            find_cut_point(&entries, 0, 3, 0),
            CutPoint {
                first_kept_index: 1,
                turn_start_index: Some(0),
                is_split_turn: true
            }
        );
    }

    #[test]
    fn prepare_compaction_domain_and_snap() {
        let system = json!({"role":"system","content":"abcd"});
        let u1 = msg_entry("u1", None, "user", &"x".repeat(400));
        let a1 = msg_entry("a1", Some("u1"), "assistant", "ok");
        let u2 = msg_entry("u2", Some("a1"), "user", "next");
        let a2 = msg_entry("a2", Some("u2"), "assistant", "four");
        let comp = json!({"type":"compaction","id":"c1","parentId":"u2","timestamp":"t","summary":"S","firstKeptEntryId":"u2","tokensBefore":1});
        let u3 = msg_entry("u3", Some("c1"), "user", "after");
        let a3 = msg_entry("a3", Some("u3"), "assistant", "done");

        // Fresh: everything before the last turn is summarized.
        let pre = prepare_compaction(&[u1.clone(), a1.clone(), u2.clone()], &system, 0)
            .expect("prepares");
        assert_eq!(pre.first_kept_entry_id, "u2");
        assert_eq!(pre.messages_to_summarize.len(), 2);
        assert!(pre.tokens_before > 0);
        assert!(pre.previous_summary.is_none());

        // Single-turn session: nothing to summarize.
        assert!(prepare_compaction(std::slice::from_ref(&u2), &system, 0).is_none());
        // Last entry is a compaction: already compacted.
        assert!(prepare_compaction(&[u1.clone(), a1.clone(), comp.clone()], &system, 0).is_none());
        // A previous compaction: domain = kept tail + after; previous summary carried;
        // a split cut snaps to the turn start (v0: no TURN_PREFIX).
        let pre = prepare_compaction(
            &[
                u1.clone(),
                a1.clone(),
                u2.clone(),
                comp.clone(),
                a2.clone(),
                u3.clone(),
                a3.clone(),
            ],
            &system,
            0,
        )
        .expect("prepares after a compaction");
        assert_eq!(pre.previous_summary.as_deref(), Some("S"));
        assert_eq!(pre.first_kept_entry_id, "u3");
        let ids: Vec<&str> = pre
            .messages_to_summarize
            .iter()
            .map(|m| {
                m.get("content")
                    .and_then(Value::as_array)
                    .unwrap()
                    .first()
                    .unwrap()
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap()
            })
            .collect();
        assert_eq!(
            ids,
            vec!["next", "four"],
            "the kept tail + the turn before the new user turn"
        );
    }

    #[test]
    fn serialize_conversation_shape_truncation_and_thinking_omitted() {
        let long = "y".repeat(2500);
        let msgs = vec![
            json!({"role":"user","content":[{"type":"text","text":"hello"}]}),
            json!({"role":"assistant","content":[
                {"type":"thinking","thinking":"secret reasoning"},
                {"type":"text","text":"hi there"},
                {"type":"toolCall","id":"t","name":"bash","input":{"command":"ls -la"}}
            ]}),
            json!({"role":"toolResult","toolCallId":"t","toolName":"bash","content":[{"type":"text","text":long}],"isError":false}),
        ];
        let out = serialize_conversation(&msgs);
        let expected = format!(
            "[User]: hello\n\n[Assistant]: hi there\n\n[Assistant tool calls]: bash(command=\"ls -la\")\n\n[Tool result]: {}\n\n[... 500 more characters truncated]",
            "y".repeat(2000)
        );
        assert_eq!(out, expected);
        assert!(!out.contains("secret reasoning"), "thinking omitted");
        // No text at all: only the tool call line.
        let calls_only = json!({"role":"assistant","content":[{"type":"toolCall","id":"t2","name":"bash","input":{"command":"ls -la"}}]});
        assert_eq!(
            serialize_conversation(&[calls_only]),
            "[Assistant tool calls]: bash(command=\"ls -la\")"
        );
    }

    #[test]
    fn project_messages_fresh_and_compacted() {
        let u1 = msg_entry("u1", None, "user", "one");
        let a1 = msg_entry("a1", Some("u1"), "assistant", "two");
        let u2 = msg_entry("u2", Some("a1"), "user", "three");
        let a2 = msg_entry("a2", Some("u2"), "assistant", "four");
        let comp = json!({"type":"compaction","id":"c1","parentId":"a2","timestamp":"t","summary":"SUMMARY","firstKeptEntryId":"u2","tokensBefore":42});
        let u3 = msg_entry("u3", Some("c1"), "user", "five");

        // Fresh: every message, in order.
        let p = project_messages(&[u1.clone(), a1.clone(), u2.clone()]);
        assert_eq!(
            p,
            [
                u1["message"].clone(),
                a1["message"].clone(),
                u2["message"].clone()
            ]
        );

        // Compacted: summary + kept (u2, a2) + after (u3).
        let p = project_messages(&[
            u1.clone(),
            a1.clone(),
            u2.clone(),
            a2.clone(),
            comp.clone(),
            u3.clone(),
        ]);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0]["role"], "compactionSummary");
        assert_eq!(p[0]["content"][0]["text"], "SUMMARY");
        assert_eq!(p[1]["content"][0]["text"], "three");
        assert_eq!(p[2]["content"][0]["text"], "four");
        assert_eq!(p[3]["content"][0]["text"], "five");
        let all = p.iter().map(|m| m.to_string()).collect::<Vec<_>>().join("");
        assert!(
            !all.contains("one") && !all.contains("two"),
            "summarized messages projected out: {all}"
        );

        // An older compaction inside the kept range contributes nothing.
        let comp_old = json!({"type":"compaction","id":"c0","parentId":"u1","timestamp":"t","summary":"OLD","firstKeptEntryId":"a1","tokensBefore":1});
        let p = project_messages(&[
            u1.clone(),
            comp_old.clone(),
            u2.clone(),
            a2.clone(),
            comp.clone(),
            u3.clone(),
        ]);
        let all = p.iter().map(|m| m.to_string()).collect::<Vec<_>>().join("");
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(
            !all.contains("OLD"),
            "older compaction contributes nothing: {all}"
        );

        // A missing firstKeptEntryId keeps nothing before the compaction.
        let comp_bad = json!({"type":"compaction","id":"c1","parentId":"a2","timestamp":"t","summary":"S","firstKeptEntryId":"nope","tokensBefore":1});
        let p = project_messages(&[u1.clone(), a1.clone(), comp_bad.clone(), u3.clone()]);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0]["role"], "compactionSummary");
        assert_eq!(p[1]["content"][0]["text"], "five");
    }

    #[test]
    fn append_compaction_entry_shape_and_projection() {
        let d = tmpdir();
        let path = d.join("comp.jsonl");
        let s = Session::create(path.clone(), "/work/comp").unwrap();
        let _id_u2 = s.append_message(
            &msg_entry("x", None, "user", "keep me")
                .get("message")
                .cloned()
                .unwrap(),
        );
        let leaf_id = s.append_message(
            &msg_entry("x", None, "assistant", "drop me")
                .get("message")
                .cloned()
                .unwrap(),
        );
        let entries = s.entries();
        let u2_id = entries[0]
            .get("id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let system = json!({"role":"system","content":"sys","toolsAdded":[],"timestamp":1});
        let entry_id = s.append_compaction("SUM", &u2_id, 77, None, false, Some(&system));
        let file = std::fs::read_to_string(&path).unwrap();
        let last: Value = serde_json::from_str(file.lines().last().unwrap()).unwrap();
        assert_eq!(last["type"], "compaction");
        assert_eq!(last["id"], entry_id);
        assert_eq!(last["parentId"], leaf_id, "child of the previous leaf");
        assert_eq!(last["summary"], "SUM");
        assert_eq!(last["firstKeptEntryId"], u2_id);
        assert_eq!(last["tokensBefore"], 77);
        assert!(
            last.get("fromHook").is_none(),
            "core compaction: no fromHook"
        );
        assert!(last.get("usage").is_none(), "zero usage: omitted");
        assert_eq!(last["systemMessage"]["content"], "sys");
        // fromHook + usage are written when given.
        let s2 = Session::create(d.join("comp2.jsonl"), "/work/comp").unwrap();
        let e2 = s2.entries()[..0].len(); // no entries: firstKept = the new entry id
        let _ = e2;
        let u = crate::model::Usage {
            input: 3,
            output: 4,
            cache_read: 0,
            cache_write: 0,
        };
        let entry_id = s2.append_compaction("S", "abc", 1, Some(&u), true, None);
        let file = std::fs::read_to_string(d.join("comp2.jsonl")).unwrap();
        let last: Value = serde_json::from_str(file.lines().last().unwrap()).unwrap();
        assert_eq!(last["fromHook"], json!(true));
        assert_eq!(
            last["usage"],
            json!({"input":3,"output":4,"cacheRead":0,"cacheWrite":0})
        );
        assert!(last.get("systemMessage").is_none());
        assert!(
            !last["id"].as_str().unwrap().is_empty() && last["id"].as_str().unwrap() != "abc",
            "firstKeptEntryId is stored verbatim"
        );
        let _ = entry_id;
        // The transcript is re-projected: compactionSummary + the kept range
        // (from firstKeptEntryId to the compaction, inclusive).
        let t = s.transcript();
        assert_eq!(t.len(), 3, "{t:?}");
        assert_eq!(t[0]["role"], "compactionSummary");
        assert_eq!(t[1]["content"][0]["text"], "keep me");
        assert_eq!(t[2]["content"][0]["text"], "drop me");
        let _ = std::fs::remove_dir_all(&d);
    }
}
