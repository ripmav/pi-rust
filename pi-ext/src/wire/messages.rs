//! LLM message + content types shared by the wire payloads (context
//! decisions, provider streaming, agent events).
//!
//! Upstream source of truth: `packages/ai/src/types.ts` at the pinned
//! commit. pi-rust's producers sometimes omit fields the upstream
//! TypeScript types mark required (e.g. a fake-driver assistant message
//! without `api`/`provider`/`model`), so — apart from the `role`/`type`
//! discriminators — every field here is optional and skipped when
//! `None`. Unknown fields are ignored on parse (serde default).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A timestamp as it appears on the wire. Upstream messages carry Unix
/// milliseconds (number); legacy pi-rust producers have emitted ISO
/// strings, so the field is untyped.
pub type JsonTimestamp = Value;

/// One content block of a message (`type`-tagged union, upstream
/// `TextContent` / `ThinkingContent` / `ImageContent` / `ToolCall`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text (upstream `TextContent`).
    #[serde(rename = "text")]
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    /// Provider reasoning block (upstream `ThinkingContent`).
    #[serde(rename = "thinking")]
    #[serde(rename_all = "camelCase")]
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redacted: Option<bool>,
    },
    /// Base64 image (upstream `ImageContent`).
    #[serde(rename = "image")]
    #[serde(rename_all = "camelCase")]
    Image { data: String, mime_type: String },
    /// A tool call the model requested (upstream `ToolCall`).
    #[serde(rename = "toolCall")]
    #[serde(rename_all = "camelCase")]
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments_error: Option<String>,
    },
}

/// `content` of a system/user message: a plain string or a block list
/// (upstream `string | TextContent[]` / `string | (TextContent|ImageContent)[]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain instruction/text.
    Text(String),
    /// Structured blocks.
    Blocks(Vec<ContentBlock>),
}

/// Token usage + cost (upstream `Usage`; fields optional because pi-rust
/// producers emit partial usage, e.g. without `cost`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

/// Catalog/provider cost breakdown (upstream `Usage["cost"]`); fields are
/// JSON numbers (int or float, verbatim).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_from_provider: Option<bool>,
}

/// A system message (upstream `SystemMessage`). `toolsAdded`/`toolsRemoved`
/// are TypeBox tool definitions — opaque JSON by design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<std::collections::BTreeMap<String, Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_added: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_removed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
}

/// A user message (upstream `UserMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
}

/// An assistant message (upstream `AssistantMessage`); the `role:
/// "assistant"` tag is carried by [`AgentMessage`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A recorded tool result (upstream `ToolResultMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_calls: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// An LLM message of any role (upstream `AgentMessage =
/// SystemMessage | UserMessage | AssistantMessage | ToolResultMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum AgentMessage {
    /// `role: "system"`.
    #[serde(rename = "system")]
    System(SystemMessage),
    /// `role: "user"`.
    #[serde(rename = "user")]
    User(UserMessage),
    /// `role: "assistant"`.
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    /// `role: "toolResult"`.
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

/// The model subset that crosses the wire (`ready.providers` /
/// `providers_changed` / `provider_stream` / `set_model`): the full
/// upstream `Model` catalog entry stays sidecar-side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The live message-so-far under a stream event's `partial`. The wire
/// carries anywhere from the full role-tagged message (e.g. `start`) to
/// a sparse object (the doc's `text_delta` example sends `partial: {}`),
/// so the sparse form is carried verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PartialMessage {
    /// A role-tagged message.
    Message(AgentMessage),
    /// Sparse/empty partial — verbatim JSON.
    Sparse(Value),
}

/// One event of an extension-registered provider stream, verbatim under
/// `provider_stream_event.event` (upstream `AssistantMessageEvent`).
/// `partial`/`message`/`error` carry the live or final assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    /// Streaming started; `partial` is the empty live message.
    Start { partial: PartialMessage },
    /// A text block started at `contentIndex`.
    #[serde(rename = "text_start")]
    #[serde(rename_all = "camelCase")]
    TextStart {
        content_index: u64,
        partial: PartialMessage,
    },
    /// Text delta.
    #[serde(rename = "text_delta")]
    #[serde(rename_all = "camelCase")]
    TextDelta {
        content_index: u64,
        delta: String,
        partial: PartialMessage,
    },
    /// Text block finished.
    #[serde(rename = "text_end")]
    #[serde(rename_all = "camelCase")]
    TextEnd {
        content_index: u64,
        content: String,
        partial: PartialMessage,
    },
    /// A thinking block started.
    #[serde(rename = "thinking_start")]
    #[serde(rename_all = "camelCase")]
    ThinkingStart {
        content_index: u64,
        partial: PartialMessage,
    },
    /// Thinking delta.
    #[serde(rename = "thinking_delta")]
    #[serde(rename_all = "camelCase")]
    ThinkingDelta {
        content_index: u64,
        delta: String,
        partial: PartialMessage,
    },
    /// Thinking block finished.
    #[serde(rename = "thinking_end")]
    #[serde(rename_all = "camelCase")]
    ThinkingEnd {
        content_index: u64,
        content: String,
        partial: PartialMessage,
    },
    /// A tool call started.
    #[serde(rename = "toolcall_start")]
    #[serde(rename_all = "camelCase")]
    ToolCallStart {
        content_index: u64,
        partial: PartialMessage,
    },
    /// Tool-call arguments delta.
    #[serde(rename = "toolcall_delta")]
    #[serde(rename_all = "camelCase")]
    ToolCallDelta {
        content_index: u64,
        delta: String,
        partial: PartialMessage,
    },
    /// Tool call finished.
    #[serde(rename = "toolcall_end")]
    #[serde(rename_all = "camelCase")]
    ToolCallEnd {
        content_index: u64,
        tool_call: ContentBlock,
        partial: PartialMessage,
    },
    /// Terminal: the stream finished normally with the final `message`.
    Done {
        reason: String,
        message: AgentMessage,
    },
    /// Terminal: the stream failed/aborted with the error `message`.
    Error { reason: String, error: AgentMessage },
}

/// A tool's final or partial result (upstream `AgentToolResult`) — the
/// `data` of `extension_tool_call` / `tool_call_request` responses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}
