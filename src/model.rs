use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// One scripted tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCallSpec {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// One scripted assistant turn: optional text plus zero or more tool calls.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCallSpec>,
    /// Token usage reported by the provider (zero for the fake driver).
    pub usage: Usage,
}

/// Token usage for one model response (provider-reported).
#[derive(Clone, Copy, Default, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }

    /// Cost in USD from per-million-token registry prices (unknown model → 0).
    pub fn cost_usd(&self, cost: Option<&crate::registry::Cost>) -> f64 {
        let Some(c) = cost else { return 0.0 };
        self.input as f64 / 1e6 * c.input
            + self.output as f64 / 1e6 * c.output
            + self.cache_read as f64 / 1e6 * c.cache_read
            + self.cache_write as f64 / 1e6 * c.cache_write
    }
}

/// Model driver: produces the next assistant turn for the conversation so far.
/// Object-safe async without a trait macro: `dyn ModelDriver` is used at the
/// agent-loop seam (fake driver in scenarios/tests, real providers otherwise).
/// Streaming events a driver reports while the assistant message is
/// built. Text and tool-call variants mirror the upstream
/// `assistantMessageEvent` wire shapes (WP-802 RPC parity); the TUI
/// consumes `TextDelta` only.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// First text chunk of the message (content index 0 in v0).
    TextStart { index: u32 },
    /// A streamed text chunk.
    TextDelta { index: u32, delta: String },
    /// Text block finished; `content` is the full block.
    TextEnd { index: u32, content: String },
    /// A tool call started (id + name known).
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
        initial_args: Value,
    },
    /// Incremental tool-call argument JSON.
    ToolCallDelta { index: u32, delta: String },
    /// A tool call finished; `tool_call` is the upstream ToolCall shape
    /// `{id, name, arguments}`.
    ToolCallEnd { index: u32, tool_call: Value },
}

pub trait ModelDriver: Send + Sync {
    /// `Err` = the run cannot finish (script exhausted or provider failure).
    /// `on_delta` is called per streamed event (TUI display; headless runs
    /// pass a no-op). Drivers that do not stream call it once with the full
    /// text as a single `TextDelta`.
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>>;
}

pub struct FakeDriver {
    turns: Mutex<VecDeque<AssistantTurn>>,
    echo: bool,
    /// WP-704: the deterministic answer to the compaction summary call
    /// (the scenario `compact_summary`; keeps scripted e2e turn slots stable).
    compact_summary: String,
}

/// WP-704: the scenario-less default for the fake's compaction summary.
pub const DEFAULT_COMPACT_SUMMARY: &str = "CONTEXT SUMMARY (fake)";

/// The marker in SUMMARIZATION_SYSTEM_PROMPT that identifies the summary call.
const SUMMARY_SYSTEM_MARKER: &str = "context summarization assistant";

impl FakeDriver {
    /// Parses scenario steps: consecutive `tool_call` steps form ONE assistant
    /// turn with multiple tool calls; a `text` step is a text-only turn.
    pub fn from_steps(steps: &[Value], compact_summary: &str) -> Result<Self> {
        let mut turns: Vec<AssistantTurn> = Vec::new();
        let mut cur: Vec<ToolCallSpec> = Vec::new();
        let mut seq = 0;
        for s in steps {
            if let Some(tc) = s.get("tool_call") {
                let name = tc
                    .get("name")
                    .and_then(Value::as_str)
                    .context("tool_call step needs a string `name`")?;
                let input = tc.get("input").cloned().unwrap_or_else(|| json!({}));
                if !input.is_object() {
                    bail!("tool_call step `input` must be an object");
                }
                seq += 1;
                cur.push(ToolCallSpec {
                    id: format!("tc_{seq}"),
                    name: name.to_string(),
                    input,
                });
            } else if let Some(text) = s.get("text").and_then(Value::as_str) {
                if !cur.is_empty() {
                    turns.push(AssistantTurn {
                        text: None,
                        tool_calls: std::mem::take(&mut cur),
                        usage: Usage::default(),
                    });
                }
                turns.push(AssistantTurn {
                    text: Some(text.to_string()),
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                });
            } else {
                bail!("model_script step must be {{\"text\": \"...\"}} or {{\"tool_call\": {{\"name\", \"input\"}}}}");
            }
        }
        if !cur.is_empty() {
            turns.push(AssistantTurn {
                text: None,
                tool_calls: cur,
                usage: Usage::default(),
            });
        }
        Ok(Self {
            turns: Mutex::new(VecDeque::from(turns)),
            echo: false,
            compact_summary: compact_summary.to_string(),
        })
    }
}

impl FakeDriver {
    /// Prompt-mode driver: no script; answers the last user message with a
    /// deterministic `echo: <text>` turn, then reports exhaustion.
    pub fn echo_mode(compact_summary: &str) -> Self {
        Self {
            turns: Mutex::new(VecDeque::new()),
            echo: true,
            compact_summary: compact_summary.to_string(),
        }
    }
}

impl ModelDriver for FakeDriver {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        Box::pin(async move {
            // WP-704: the summary call is detected by its system prompt (the
            // verbatim SUMMARIZATION_SYSTEM_PROMPT marker); it answers with
            // the compact_summary as plain text, no tool calls, WITHOUT
            // consuming a scripted turn (turn slots stay stable).
            let is_summary_call = convo.iter().any(|m| {
                m.get("role").and_then(Value::as_str) == Some("system")
                    && m.get("content")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.contains(SUMMARY_SYSTEM_MARKER))
            });
            if is_summary_call {
                let text = self.compact_summary.clone();
                on_delta(StreamDelta::TextDelta {
                    index: 0,
                    delta: text.clone(),
                });
                return Ok(AssistantTurn {
                    text: Some(text),
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                });
            }
            if let Some(t) = self.turns.lock().unwrap().pop_front() {
                if let Some(text) = &t.text {
                    on_delta(StreamDelta::TextDelta {
                        index: 0,
                        delta: text.to_string(),
                    }); // single delta: the fake does not stream
                }
                return Ok(t);
            }
            if self.echo {
                let last_user = convo
                    .iter()
                    .rev()
                    .find(|m| m.get("role").and_then(Value::as_str) == Some("user"));
                let text = last_user
                    .map(crate::session::extract_text)
                    .unwrap_or_default();
                let reply = format!("echo: {text}");
                on_delta(StreamDelta::TextDelta {
                    index: 0,
                    delta: reply.clone(),
                });
                return Ok(AssistantTurn {
                    text: Some(reply),
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                });
            }
            anyhow::bail!("model script exhausted before the run could finish")
        })
    }
}

#[cfg(test)]
mod wp704_tests {
    use super::*;

    fn summary_convo() -> Vec<Value> {
        vec![
            json!({"role":"system","content":"You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."}),
            json!({"role":"user","content":[{"type":"text","text":"<conversation>\n[User]: hi\n</conversation>"}]}),
        ]
    }

    #[test]
    fn summary_call_is_detected_and_does_not_consume_script() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let driver = FakeDriver::from_steps(&[json!({"text": "scripted"})], "MY SUMMARY").unwrap();
        // The summary call answers with the scenario's compact_summary.
        let t1 = rt
            .block_on(driver.complete(&summary_convo(), Box::new(|_| {})))
            .unwrap();
        assert_eq!(t1.text.as_deref(), Some("MY SUMMARY"));
        assert!(t1.tool_calls.is_empty(), "plain text, no tool calls");
        // The script is intact: the next ordinary call pops the scripted turn.
        let normal = vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})];
        let t2 = rt
            .block_on(driver.complete(&normal, Box::new(|_| {})))
            .unwrap();
        assert_eq!(t2.text.as_deref(), Some("scripted"));
        // echo mode: the configured summary (cli passes the default marker).
        let echo = FakeDriver::echo_mode(DEFAULT_COMPACT_SUMMARY);
        let t3 = rt
            .block_on(echo.complete(&summary_convo(), Box::new(|_| {})))
            .unwrap();
        assert_eq!(t3.text.as_deref(), Some(DEFAULT_COMPACT_SUMMARY));
    }
}
