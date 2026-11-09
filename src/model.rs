use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Mutex;

/// One scripted tool call.
#[derive(Clone)]
pub struct ToolCallSpec {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// One scripted assistant turn: optional text plus zero or more tool calls.
#[derive(Clone)]
pub struct AssistantTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCallSpec>,
}

/// Scripted model driver: emits turns in the scripted order.
pub trait ModelDriver: Send + Sync {
    /// Next scripted turn; `None` = script exhausted (a model script error).
    fn next_turn(&self) -> Option<AssistantTurn>;
}

pub struct FakeDriver {
    turns: Mutex<VecDeque<AssistantTurn>>,
}

impl FakeDriver {
    /// Parses scenario steps: consecutive `tool_call` steps form ONE assistant
    /// turn with multiple tool calls; a `text` step is a text-only turn.
    pub fn from_steps(steps: &[Value]) -> Result<Self> {
        let mut turns: Vec<AssistantTurn> = Vec::new();
        let mut cur: Vec<ToolCallSpec> = Vec::new();
        let mut seq = 0;
        for s in steps {
            if let Some(tc) = s.get("tool_call") {
                let name = tc.get("name").and_then(Value::as_str).context("tool_call step needs a string `name`")?;
                let input = tc.get("input").cloned().unwrap_or_else(|| json!({}));
                if !input.is_object() {
                    bail!("tool_call step `input` must be an object");
                }
                seq += 1;
                cur.push(ToolCallSpec { id: format!("tc_{seq}"), name: name.to_string(), input });
            } else if let Some(text) = s.get("text").and_then(Value::as_str) {
                if !cur.is_empty() {
                    turns.push(AssistantTurn { text: None, tool_calls: std::mem::take(&mut cur) });
                }
                turns.push(AssistantTurn { text: Some(text.to_string()), tool_calls: Vec::new() });
            } else {
                bail!("model_script step must be {{\"text\": \"...\"}} or {{\"tool_call\": {{\"name\", \"input\"}}}}");
            }
        }
        if !cur.is_empty() {
            turns.push(AssistantTurn { text: None, tool_calls: cur });
        }
        Ok(Self { turns: Mutex::new(VecDeque::from(turns)) })
    }
}

impl ModelDriver for FakeDriver {
    fn next_turn(&self) -> Option<AssistantTurn> {
        self.turns.lock().unwrap().pop_front()
    }
}
