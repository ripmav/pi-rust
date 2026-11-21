//! Catalog event payloads and decision-result types
//! (docs/bridge-protocol.md "Message catalog" + "Decision-event
//! semantics"). Each event struct mirrors the upstream event object under
//! the command's `event` key (or the standalone sidecar→core line); the
//! inner `type` field is the upstream discriminant and is carried
//! verbatim.
//!
//! Field tolerance rule (see `messages` module docs): apart from the
//! discriminants, fields are optional and skipped when `None`, so both
//! the sidecar's full upstream objects and the core's documented subsets
//! parse.

use crate::wire::messages::{
    AgentMessage, AssistantMessageEvent, ContentBlock, JsonTimestamp, ModelRef, Usage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// =====================================================================
// core→sidecar event payloads (under `Command` variants' `event`)
// =====================================================================

/// `session_start` (upstream `SessionStartEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStartEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_session_file: Option<String>,
}

/// `before_agent_start` trigger (upstream
/// `BeforeAgentStartUserEvent` / `BeforeAgentStartCustomEvent` minus the
/// `systemPrompt` getter, which crosses as a top-level command field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartEvent {
    #[serde(rename = "type")]
    pub kind: String,
    /// `"user"` (raw `prompt`/`images`) or `"custom"` (`message`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ContentBlock>>,
    /// The custom message for `messageType: "custom"` (upstream
    /// `CustomMessage` pick) — opaque JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
}

/// `BuildSystemPromptOptions` JSON subset the core sends with
/// `before_agent_start` (upstream `core/system-prompt.ts`): the handler
/// mutates these in place; `sections` values of `null` remove a section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemPromptOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<std::collections::BTreeMap<String, Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Value>,
}

/// `agent_start` (upstream `AgentStartEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStartEvent {
    #[serde(rename = "type")]
    pub kind: String,
}

/// `agent_end` (upstream `AgentEndEvent`); `willRetry` is a pi-rust
/// addition on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub messages: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
}

/// `turn_start` (upstream `TurnStartEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
}

/// `turn_end` (upstream `TurnEndEvent` without the `context` preview;
/// boundary `entries`/`continue` mutation is out of scope in v0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Vec<AgentMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_entry_ids: Option<Vec<String>>,
}

/// `message_start` (upstream `MessageStartEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: AgentMessage,
}

/// `message_update` (upstream `MessageUpdateEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUpdateEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: AgentMessage,
    pub assistant_message_event: AssistantMessageEvent,
}

/// `message_end` (upstream `MessageEndEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageEndEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: AgentMessage,
}

/// `tool_call` (upstream `ToolCallEvent`; the per-tool `input` types are
/// `Record<string, unknown>` on the wire — a handler may mutate them in
/// place, the response carries the final `input`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    pub tool_name: String,
    pub input: Value,
}

/// `tool_result` (upstream `ToolResultEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    pub tool_name: String,
    pub input: Value,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// `context` (upstream `ContextEvent`) — always answered `data: null` in
/// v0 (no-op).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub messages: Vec<AgentMessage>,
}

/// `context_with_system` (upstream `ContextWithSystemEvent`) — the single
/// phase that runs the handlers; the returned conversation is used as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextWithSystemEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub messages: Vec<AgentMessage>,
}

/// `input` (upstream `InputEvent` with `source: "rpc"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_behavior: Option<String>,
}

/// `user_bash` (upstream `UserBashEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBashEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Compaction preparation on the wire: the core's documented subset
/// (`reason`/`willRetry`) or the full upstream `CompactionPreparation`
/// (the extra fields parse and round-trip).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactPreparation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_to_summarize: Option<Vec<AgentMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_prefix_messages: Option<Vec<AgentMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_split_turn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_summary: Option<String>,
    /// Upstream `FileOperations` — opaque JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_ops: Option<Value>,
    /// Upstream `CompactionSettings` — opaque JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

/// `session_before_compact` (upstream `SessionBeforeCompactEvent` minus
/// the non-JSON `signal`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub preparation: CompactPreparation,
    /// The messages about to be summarized.
    pub branch_entries: Vec<AgentMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
}

/// A persisted compaction session entry (upstream `CompactionEntry`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    #[serde(rename = "type")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<JsonTimestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// `true` when generated by a `session_before_compact` handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_message: Option<crate::wire::messages::SystemMessage>,
}

/// `session_compact` (upstream `SessionCompactEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub compaction_entry: CompactionEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_extension: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
}

/// `session_compact_failed` (upstream `SessionCompactFailedEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactFailedEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aborted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub will_retry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_extension: Option<bool>,
}

/// `session_shutdown` (upstream `SessionShutdownEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionShutdownEvent {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_file: Option<String>,
}

/// `set_model` (upstream `ModelSelectEvent`; the `model` fields crossing
/// the wire are the [`ModelRef`] subset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub model: ModelRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// `set_thinking_level` (upstream `ThinkingLevelSelectEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelSelectEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_level: Option<String>,
}

// =====================================================================
// sidecar→core event payloads (standalone `SidecarEvent` lines)
// =====================================================================

/// A handler threw inside the sidecar (upstream `ExtensionError`). The
/// in-flight decision still resolves to its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionError {
    pub extension_path: String,
    pub event: String,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

/// `session_info_changed` (upstream `SessionInfoChangedEvent`) — the
/// session name; `null`/absent when the name is cleared.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionInfoChanged {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

// =====================================================================
// Decision-response `data` types
// =====================================================================

/// `tool_call` response `data`: the combined handler result and the
/// final (possibly mutated) input the core must execute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolCallEventResult>,
    pub input: Value,
}

/// What a `tool_call` handler decided (upstream `ToolCallEventResult`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEventResult {
    /// Block execution; the core records `Blocked by extension: <reason>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Honored only when every finalized result in the batch carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// What a `tool_result` handler changed (upstream `ToolResultEventResult`);
/// `None` = no change. Replacing `content` without `structuredContent`
/// drops the structured content (upstream rule).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// `before_agent_start` response `data` (upstream
/// `BeforeAgentStartEventResult` under `messages`): custom messages to
/// prepend and, when present, the replaced system prompt for this turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// `input` response `data` (upstream `InputEventResult`): transforms
/// chain, `handled` short-circuits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum InputEventResult {
    /// Proceed with the original input.
    Continue,
    /// Use the returned `text`/`images`.
    Transform {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ContentBlock>>,
    },
    /// Drop the input; no agent turn.
    Handled,
}

/// `user_bash` response `data` (upstream `UserBashEventResult`):
/// `result` → the core uses the [`BashResult`] verbatim; `operations` is
/// not representable in the core and means "execute locally". `None`
/// (absent) → execute locally.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBashEventResult {
    /// Upstream `BashOperations` — opaque JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BashResult>,
}

/// A bash execution result (upstream `BashResult`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    /// Combined stdout + stderr (sanitized, possibly truncated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Process exit code (absent when killed/cancelled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Temp file with the full output when truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// `session_before_compact` response `data`: `null` (proceed),
/// `{cancel: true}` (abort) or `{compaction: …}` (extension-supplied
/// summary).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionSummary>,
}

/// The extension-supplied summary (upstream
/// `SessionBeforeCompactResult["compaction"]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummary {
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

// =====================================================================
// Extension UI (dialogs) + custom panes (v6)
// =====================================================================

/// A dialog request from the sidecar (upstream `RpcExtensionUIRequest`);
/// `select`/`confirm`/`input`/`editor` are req/resp, the rest F&F.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum UiRequest {
    /// Single choice from a list.
    Select {
        id: String,
        title: String,
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    /// Yes/no confirmation.
    Confirm {
        id: String,
        title: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    /// Free-text input.
    Input {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    /// Multi-line editor.
    Editor {
        id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefill: Option<String>,
    },
    /// User-facing notification (F&F).
    #[serde(rename_all = "camelCase")]
    Notify {
        id: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notify_type: Option<String>,
    },
    /// Status-bar text (F&F); `null` clears.
    #[serde(rename_all = "camelCase")]
    SetStatus {
        id: String,
        status_key: String,
        status_text: Option<String>,
    },
    /// Widget lines (F&F); `null` clears.
    #[serde(rename_all = "camelCase")]
    SetWidget {
        id: String,
        widget_key: String,
        widget_lines: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        widget_placement: Option<String>,
    },
    /// Window title (F&F).
    SetTitle { id: String, title: String },
    /// Editor text (F&F).
    #[serde(rename = "set_editor_text")]
    SetEditorText { id: String, text: String },
}

/// A pane's `blocked` descriptor (v6 custom pane).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneBlocked {
    pub kind: String,
    pub message: String,
}

/// One line of a pane descriptor (v6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLine {
    pub text: String,
}

/// A list element of a pane descriptor (v6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneList {
    pub items: Vec<String>,
    /// 0-based cursor position.
    pub cursor: u64,
}

/// The single input element of a pane descriptor (v6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneInput {
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// A TUI key for an open pane (v6 `pane_key`): `code` is
/// `up | down | left | right | enter | escape | backspace | tab | char`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneKeyInput {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
