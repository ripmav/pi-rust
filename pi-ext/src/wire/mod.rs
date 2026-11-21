//! Bridge protocol v6 wire types — one source of truth for the core's
//! server side and the extension's client side (ADR 0003).
//!
//! Framing (docs/bridge-protocol.md §Framing): NDJSON over stdio, one
//! [`BridgeLine`] per line. Core→sidecar lines are [`Command`]s
//! (`{ id, type, …payload }`, `id` = UUID from the core — except
//! `extension_ui_response`/`pane_key`, whose `id` is the dialog/pane id
//! they reply to). Sidecar→core lines are [`Response`]s to commands
//! (`{ id, type: "response", command, success, data? | error? }`) or
//! standalone [`SidecarEvent`]s (no `id`, no response expected).
//! `stdout` is protocol output only; logs go to stderr.
//!
//! Field tolerance rule (see `messages` module docs): apart from the
//! discriminants, fields are optional and skipped when `None`, so both
//! the sidecar's full upstream objects and the core's documented subsets
//! parse and round-trip. Unknown fields are ignored on parse.

mod events;
mod messages;

pub use events::*;
pub use messages::*;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line on the bridge: a core→sidecar command, a sidecar→core
/// response, or a sidecar→core standalone event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BridgeLine {
    /// Core→sidecar (`{ id, type, …payload }`).
    Command(Command),
    /// Sidecar→core response to a command.
    Response(Response),
    /// Sidecar→core standalone event (no `id`).
    Event(SidecarEvent),
}

/// The blocking-command / dialog timeout negotiated in `init`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// The `extension_ui_request` methods the core implements
    /// (`select`, `confirm`, `input`, `editor`, `notify`; the TUI adds
    /// `custom`). Empty = headless mode (upstream noOp UI context).
    pub ui: Vec<String>,
    /// Milliseconds; omitted dialog timeouts fall back to it.
    pub timeout_ms: u64,
}

/// An MCP server config entry (upstream `McpServerConfig`, the JSON
/// fields that cross the wire; stdio = `command`/`args`/`env`, remote =
/// `url`/`headers`; `$ENV_VAR` expansion happens sidecar-side).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `direct` | `codemode` | `deferred` (upstream `McpExposure`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<String>,
    /// Per-tool exposure overrides — opaque JSON (accepted by validation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_exposure: Option<Value>,
    /// OAuth fields (v0: accepted by validation, not wired) — opaque JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Value>,
}

/// The `mcpConfig` `init` field: the agent-dir `mcp.json` merged with the
/// trusted project `.pi/mcp.json` (WP-1103), passed verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    pub mcp_servers: std::collections::BTreeMap<String, McpServerConfig>,
}

/// An extension tool's `ready`/`tools_changed` entry (subset of upstream
/// `ToolDefinition`; `parameters` is the TypeBox schema as JSON — opaque
/// by design).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyTool {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    pub parameters: Value,
    /// `direct` | `model-only` | `codemode` | `deferred` (v5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_active: Option<bool>,
}

/// A `ready.extensions` entry (subset of upstream `Extension`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyExtension {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// A `ready.errors` entry (upstream `LoadExtensionsResult["errors"]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadError {
    pub path: String,
    pub error: String,
}

/// A `ready.commands` entry (upstream `RegisteredCommand` minus the
/// handler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A `ready.shortcuts` entry (upstream-resolved `ExtensionShortcut`
/// minus the handler, v4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyShortcut {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub extension_path: String,
}

/// A `ready.flags` entry (upstream `ExtensionFlag`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionFlag {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `"boolean"` or `"string"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// `boolean | string` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    pub extension_path: String,
}

/// One extension-registered provider (v1 `ready.providers` /
/// `providers_changed` shape; the provider object itself stays
/// sidecar-side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    pub models: Vec<ModelRef>,
}

/// The `ready` event payload: the extension/tool/command registry
/// snapshot + the `events` the sidecar has handlers for (the core only
/// sends those).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ready {
    pub api_version: u64,
    pub extensions: Vec<ReadyExtension>,
    pub errors: Vec<LoadError>,
    pub tools: Vec<ReadyTool>,
    pub commands: Vec<ReadyCommand>,
    /// v4; absent in pre-v4 sidecars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<Vec<ReadyShortcut>>,
    /// v4 conflict messages (the core logs them on stderr).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut_diagnostics: Option<Vec<String>>,
    pub flags: Vec<ExtensionFlag>,
    pub providers: Vec<Provider>,
    /// Event names with ≥1 registered handler.
    pub events: Vec<String>,
    /// The names active at ready (v5; see "Active tool set").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tools: Option<Vec<String>>,
}

/// `provider_stream` sampling options (upstream `StreamOptions` subset).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStreamOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// Core→sidecar command lines, `type`-tagged. Every variant carries the
/// framing `id` (UUID from the core — except `ExtensionUiResponse` and
/// `PaneKey`, whose `id` is the dialog/pane id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Handshake: `protocolVersion: 6` + capability set. `projectTrusted`
    /// (v6): the core's resolved project-trust decision — absent =
    /// untrusted, gates project-scope extension loading.
    #[serde(rename_all = "camelCase")]
    Init {
        id: String,
        protocol_version: u64,
        capabilities: Capabilities,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mcp_config: Option<McpConfig>,
        /// The core's resolved keybinding table (action → keys, v4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keybindings: Option<std::collections::BTreeMap<String, Vec<String>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_trusted: Option<bool>,
    },
    /// Session started/loaded/reloaded (F&F).
    SessionStart {
        id: String,
        event: SessionStartEvent,
    },
    /// Before the run starts (decision): the `data` carries
    /// [`BeforeAgentStartResult`]; `systemPrompt` is the currently
    /// rendered prompt (top-level, not under `event`).
    #[serde(rename_all = "camelCase")]
    BeforeAgentStart {
        id: String,
        event: BeforeAgentStartEvent,
        system_prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt_options: Option<SystemPromptOptions>,
    },
    /// Agent loop started (F&F).
    AgentStart { id: String, event: AgentStartEvent },
    /// Agent loop ended (F&F).
    AgentEnd { id: String, event: AgentEndEvent },
    /// Turn started (F&F).
    TurnStart { id: String, event: TurnStartEvent },
    /// Turn ended (F&F in v0; boundary mutation out of scope).
    TurnEnd { id: String, event: TurnEndEvent },
    /// Message started (F&F).
    MessageStart {
        id: String,
        event: MessageStartEvent,
    },
    /// Streaming token update (F&F).
    MessageUpdate {
        id: String,
        event: MessageUpdateEvent,
    },
    /// Message finished (F&F in v0; replacement out of scope).
    MessageEnd { id: String, event: MessageEndEvent },
    /// Before a tool executes (decision): `data` = [`ToolCallResponse`]
    /// (final input + optional block/reason).
    ToolCall { id: String, event: ToolCallEvent },
    /// After a tool ran (decision): `data` = [`ToolResultEventResult`]
    /// or `null`.
    ToolResult { id: String, event: ToolResultEvent },
    /// Conversation before the LLM call, no system messages (decision;
    /// answered `data: null` in v0).
    Context { id: String, event: ContextEvent },
    /// Full transcript incl. system messages (decision): the returned
    /// conversation is used as-is.
    ContextWithSystem {
        id: String,
        event: ContextWithSystemEvent,
    },
    /// User input before agent processing (decision): `data` =
    /// [`InputEventResult`].
    Input { id: String, event: InputEvent },
    /// User ran a bash command (decision): `data` =
    /// [`UserBashEventResult`] or `null` (execute locally).
    UserBash { id: String, event: UserBashEvent },
    /// Before compaction (decision): `data` =
    /// [`SessionBeforeCompactResult`] or `null` (proceed).
    SessionBeforeCompact {
        id: String,
        event: SessionBeforeCompactEvent,
    },
    /// Compaction succeeded (F&F).
    SessionCompact {
        id: String,
        event: SessionCompactEvent,
    },
    /// Compaction failed (F&F).
    SessionCompactFailed {
        id: String,
        event: SessionCompactFailedEvent,
    },
    /// Session shutting down (F&F).
    SessionShutdown {
        id: String,
        event: SessionShutdownEvent,
    },
    /// Invoke a registered extension command; `data` = the handler's
    /// JSON return value or `null`; unknown name → error response.
    Command {
        id: String,
        name: String,
        args: String,
    },
    /// Dispatch a keypress to a resolved extension shortcut (v4);
    /// `data` = the handler's JSON return value or `null`.
    Shortcut { id: String, key: String },
    /// Execute a registered extension tool; `data` = [`AgentToolResult`].
    #[serde(rename_all = "camelCase")]
    ExtensionToolCall {
        id: String,
        tool_call_id: String,
        tool_name: String,
        input: Value,
    },
    /// Invoke an extension-registered provider (v1): the sidecar acks,
    /// then emits one `provider_stream_event` line per upstream event;
    /// the terminal `done`/`error` ends the stream.
    #[serde(rename_all = "camelCase")]
    ProviderStream {
        id: String,
        stream_id: String,
        model: ModelRef,
        messages: Vec<AgentMessage>,
        options: ProviderStreamOptions,
    },
    /// Model selected (F&F).
    SetModel { id: String, event: ModelSelectEvent },
    /// Thinking level selected (F&F).
    SetThinkingLevel {
        id: String,
        event: ThinkingLevelSelectEvent,
    },
    /// Reply to a dialog: `id` is the dialog id from the
    /// `extension_ui_request`; exactly one of `value`/`confirmed`/
    /// `cancelled` is set.
    ExtensionUiResponse {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confirmed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancelled: Option<bool>,
    },
    /// A TUI key for an open pane (v6); `id` is the pane id.
    PaneKey { id: String, key: PaneKeyInput },
}

/// Sidecar→core response to a command. `id` is `null` on the parse-error
/// response; unknown/already-timed-out ids are ignored by the core.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Always `"response"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The command this responds to.
    pub command: String,
    pub success: bool,
    /// Present on `success: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Present on `success: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sidecar→core standalone events, `type`-tagged (no `id`, no response
/// expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidecarEvent {
    /// Handshake complete: the registry snapshot.
    Ready(Ready),
    /// A handler threw; the in-flight decision resolves to its default.
    ExtensionError(ExtensionError),
    /// A dialog request (req/resp for the dialog subset, F&F otherwise).
    ExtensionUiRequest(UiRequest),
    /// The session name changed.
    SessionInfoChanged(SessionInfoChanged),
    /// The sidecar appends a session entry — `entry` is the session
    /// entry JSON (core `session-manager` shape, nested under `entry` so
    /// `entry.type` cannot clobber the line discriminator); the core is
    /// the source of truth and persists it.
    SessionAppendEntry { entry: Value },
    /// `pi.sendUserMessage` / `pi.sendMessage`: the core enqueues a user
    /// turn (steers when a turn is active).
    UserMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        images: Option<Vec<ContentBlock>>,
    },
    /// `pi.setModel`: the core switches the active model (`data:
    /// { ok }`); provider included — model ids are not unique across
    /// providers.
    #[serde(rename_all = "camelCase")]
    SetModelRequest { model_id: String, provider: String },
    /// `pi.setThinkingLevel`: the core switches the level (`data:
    /// { ok }`), then echoes `set_thinking_level` to extensions.
    SetThinkingLevelRequest { level: String },
    /// Nested `ctx.executeTool`: the core executes the named tool with
    /// its normal hooks; `data` = [`AgentToolResult`].
    #[serde(rename_all = "camelCase")]
    ToolCallRequest {
        tool_call_id: String,
        tool_name: String,
        input: Value,
    },
    /// The extension tool set changed post-ready (v2): the core
    /// **replaces** its set; coalesced to one per sidecar tick, followed
    /// by `tools_activated` for the same tick (v5).
    ToolsChanged { tools: Vec<ReadyTool> },
    /// The active tool set changed (v5): the core **replaces** its
    /// active set.
    ToolsActivated { active: Vec<String> },
    /// A provider registered/unregistered post-ready (v1): the core
    /// re-keys per-turn routing from the payload.
    ProvidersChanged { providers: Vec<Provider> },
    /// One event of a `provider_stream` invocation (v1); `done`/`error`
    /// is terminal for the `streamId`.
    #[serde(rename_all = "camelCase")]
    ProviderStreamEvent {
        stream_id: String,
        event: AssistantMessageEvent,
    },
    /// A custom pane opened (v6).
    PaneOpen {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked: Option<PaneBlocked>,
    },
    /// A full pane descriptor snapshot (v6, replacement not patch).
    PaneUpdate {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<Vec<PaneLine>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        list: Option<PaneList>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<PaneInput>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked: Option<PaneBlocked>,
    },
    /// The pane's component called `done(result)` (v6); the result
    /// resolves sidecar-side and does not cross the wire.
    PaneClose { id: String },
}
