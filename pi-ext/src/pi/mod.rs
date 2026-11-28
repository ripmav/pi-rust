//! The extension API core — the 1:1 Rust mirror of the upstream `pi`
//! object + `ctx` (ADR 0002), limited to what the bridge v6 catalog can
//! express: the [`Extension`] trait (init entry + event handlers), the
//! context ([`ExtensionContext`]), tool registration ([`Tool`],
//! [`schema`] helpers), commands/shortcuts/flags ([`Command`],
//! [`Shortcut`], [`Flag`]), provider and MCP registration
//! ([`Provider`], [`McpServer`]), the UI surface ([`ExtensionUI`] on
//! `ctx.ui`, [`NullUi`] = the no-UI default) and the
//! message/session/model surface on [`Pi`].
//!
//! The transports (v6 user process, core cdylib) land in WP-1205/1206;
//! registrations are stored on [`Pi`] (accessors below) so they can
//! dispatch them, and the catalog ↔ API parity test (`tests/parity.rs`)
//! keeps the mirror honest.
//!
//! # Example ("hello tool")
//!
//! ```
//! use pi_ext::pi::{Extension, ExtensionContext, Pi, SessionStartEvent, Tool, ToolContext};
//! use pi_ext::wire::AgentToolResult;
//! use serde_json::Value;
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn init(&mut self, pi: &mut Pi) {
//!         pi.register_tool(Tool::new(
//!             "hello",
//!             "Say hello to someone",
//!             pi_ext::pi::schema::object(
//!                 &[("name", pi_ext::pi::schema::string(Some("Who to greet")))],
//!                 &["name"],
//!             ),
//!             |input: Value, _ctx: &ToolContext| Box::pin(async move {
//!                 let name = input["name"].as_str().unwrap_or("world");
//!                 Ok(AgentToolResult::text(format!("Hello, {name}!")))
//!             }),
//!         ));
//!     }
//!
//!     fn on_session_start(&self, _event: &SessionStartEvent, _ctx: &ExtensionContext) {
//!         eprintln!("hello: session started");
//!     }
//! }
//! # let _ = Hello;
//! ```
//!
//! Runtime actions (messages, session, model, UI) run through the
//! [`ExtensionTransport`] supplied by the runtime — [`NullTransport`]
//! no-ops them (and carries the [`NullUi`] no-UI default), which is what
//! a standalone compile/test needs:
//!
//! ```
//! use pi_ext::pi::{
//!     Command, Extension, ExtensionContext, Flag, FlagKind, McpServer, NullTransport,
//!     Pi, Provider, ProviderConfig, ProviderModel, Tool, ToolContext,
//! };
//! use pi_ext::wire::{AgentToolResult, McpServerConfig, MessageContent};
//! use serde_json::{json, Value};
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn init(&mut self, pi: &mut Pi) {
//!         pi.register_tool(Tool::new(
//!             "hello",
//!             "Say hello to someone",
//!             pi_ext::pi::schema::object(
//!                 &[("name", pi_ext::pi::schema::string(Some("Who to greet")))],
//!                 &["name"],
//!             ),
//!             |input: Value, _ctx: &ToolContext| Box::pin(async move {
//!                 let name = input["name"].as_str().unwrap_or("world");
//!                 Ok(AgentToolResult::text(format!("Hello, {name}!")))
//!             }),
//!         ));
//!         pi.register_command(Command::new("hello", |args: String, _ctx: &ExtensionContext| {
//!             Box::pin(async move { Some(json!({ "args": args })) })
//!         }));
//!         pi.register_flag(Flag::new("greet", FlagKind::String).with_default(json!("world")));
//!         pi.register_provider(Provider::new(
//!             "faketest",
//!             ProviderConfig::new(
//!                 vec![ProviderModel::new("faketest/echo-1")],
//!                 |_model, _messages, _options, _emit| Box::pin(async {}),
//!             ),
//!         ));
//!         pi.register_mcp_server(McpServer::new("docs", McpServerConfig::default()));
//!     }
//! }
//!
//! let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
//! let mut hello = Hello;
//! hello.init(&mut pi);
//! pi.send_user_message(MessageContent::Text("hi".into()), None);
//! pi.append_entry("my-state", json!({"step": 1}));
//! // No --greet on the command line → the registered default.
//! assert_eq!(pi.get_flag("greet"), Some(json!("world")));
//! assert_eq!(pi.get_flag("nope"), None);
//! ```

mod command;
mod provider;
mod tool;
mod ui;

pub use command::{Command, CommandHandler, Flag, FlagKind, Shortcut, ShortcutHandler};
pub use provider::{McpServer, Provider, ProviderConfig, ProviderModel, ProviderStreamFn};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
pub use tool::{schema, Tool, ToolContext, ToolExecutor, ToolOutcome};
pub use ui::{
    ExtensionUI, NotifyKind, NullUi, Pane, PaneDescriptor, PaneFactory, PaneHost, UiDialogOptions,
};

/// The extension context (upstream `ExtensionContext`, the wire-
/// expressible subset per ADR 0002): the run mode, dialog availability,
/// working directory, the session/model view and the UI surface.
#[derive(Clone)]
pub struct ExtensionContext {
    /// `"tui"` | `"rpc"` | `"json"` | `"print"`.
    pub mode: String,
    /// Dialog-capable UI available (`ui` capability non-empty).
    pub has_ui: bool,
    /// Current working directory.
    pub cwd: String,
    /// The session display name, if set.
    pub session_name: Option<String>,
    /// The active model, when known.
    pub model: Option<ModelRef>,
    /// The current thinking level, when the session runtime provides one.
    pub thinking_level: Option<String>,
    /// The UI surface (upstream `ctx.ui`); [`NullUi`] = the headless
    /// no-UI default. [`Pi::new`] wires it from the transport.
    pub ui: Arc<dyn ExtensionUI>,
}

impl Default for ExtensionContext {
    fn default() -> Self {
        Self {
            mode: String::new(),
            has_ui: false,
            cwd: String::new(),
            session_name: None,
            model: None,
            thinking_level: None,
            ui: Arc::new(NullUi),
        }
    }
}

impl std::fmt::Debug for ExtensionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionContext")
            .field("mode", &self.mode)
            .field("has_ui", &self.has_ui)
            .field("cwd", &self.cwd)
            .field("session_name", &self.session_name)
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("ui", &"<ExtensionUI>")
            .finish()
    }
}

impl PartialEq for ExtensionContext {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.has_ui == other.has_ui
            && self.cwd == other.cwd
            && self.session_name == other.session_name
            && self.model == other.model
            && self.thinking_level == other.thinking_level
    }
}

/// The result of `exec` (upstream `ExecResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    /// Exit code, or 128 + signal number when signal-terminated.
    pub code: i32,
    /// Terminated by pi: timeout, abort, or output over the buffer limit.
    pub killed: bool,
    /// Output exceeded the buffer limit; stdout/stderr are incomplete.
    pub truncated: bool,
}

/// Options for [`Pi::send_user_message`] (upstream `sendUserMessage`
/// options).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SendUserMessageOptions {
    /// How to queue the message while the agent is streaming.
    /// `"steer"` | `"followUp"`.
    pub deliver_as: Option<String>,
    /// Dispatch extension commands and expand skill commands / prompt
    /// templates.
    pub expand_prompt_templates: Option<bool>,
}

/// The runtime hook behind [`Pi`]'s actions and the tool's nested
/// `executeTool`. Implemented by the transports (WP-1205 v6 process,
/// WP-1206 core cdylib); [`NullTransport`] is the no-op stand-in for
/// standalone compiles and tests.
pub trait ExtensionTransport: Send + Sync {
    /// `pi.sendUserMessage` — the core enqueues a user turn.
    fn send_user_message(&self, content: MessageContent, options: Option<SendUserMessageOptions>);
    /// `pi.appendEntry` — persist a custom entry (not sent to the LLM).
    fn append_entry(&self, custom_type: &str, data: Value);
    /// `pi.setSessionName`.
    fn set_session_name(&self, name: &str);
    /// `pi.setLabel` — `None` clears the label.
    fn set_label(&self, entry_id: &str, label: Option<&str>);
    /// `pi.getSettings` — effective settings, as JSON.
    fn get_settings(&self) -> Value;
    /// `pi.setThinkingLevel`.
    fn set_thinking_level(&self, level: &str);
    /// `pi.setModel` — resolves `true` when the switch happened.
    fn set_model(&self, model: ModelRef) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
    /// `pi.exec`.
    fn exec(
        &self,
        command: &str,
        args: &[String],
    ) -> Pin<Box<dyn Future<Output = ExecResult> + Send + '_>>;
    /// Nested `ctx.executeTool` from a tool handler — runs the named tool
    /// with the core's normal hooks; failures come back as `Err`.
    fn execute_tool(
        &self,
        name: &str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, String>> + Send + '_>>;
    /// `ctx.ui` — the target's UI surface ([`NullUi`] headless).
    fn ui(&self) -> Arc<dyn ExtensionUI>;
}

/// A no-op [`ExtensionTransport`] for standalone compiles, doc examples
/// and unit tests.
#[derive(Default)]
pub struct NullTransport;

impl ExtensionTransport for NullTransport {
    fn send_user_message(
        &self,
        _content: MessageContent,
        _options: Option<SendUserMessageOptions>,
    ) {
    }
    fn append_entry(&self, _custom_type: &str, _data: Value) {}
    fn set_session_name(&self, _name: &str) {}
    fn set_label(&self, _entry_id: &str, _label: Option<&str>) {}
    fn get_settings(&self) -> Value {
        Value::Object(Default::default())
    }
    fn set_thinking_level(&self, _level: &str) {}
    fn set_model(&self, _model: ModelRef) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async { true })
    }
    fn exec(
        &self,
        _command: &str,
        _args: &[String],
    ) -> Pin<Box<dyn Future<Output = ExecResult> + Send + '_>> {
        Box::pin(async {
            ExecResult {
                stdout: String::new(),
                stderr: "NullTransport: exec not wired".into(),
                code: 1,
                killed: false,
                truncated: false,
            }
        })
    }
    fn execute_tool(
        &self,
        name: &str,
        _args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, String>> + Send + '_>> {
        let name = name.to_string();
        Box::pin(async move { Err(format!("NullTransport: {name} not wired")) })
    }
    fn ui(&self) -> Arc<dyn ExtensionUI> {
        Arc::new(NullUi)
    }
}

/// The `pi` object handed to [`Extension::init`] and used from handlers:
/// registrations collected during `init`, runtime actions through the
/// transport.
pub struct Pi {
    /// The context for this extension instance.
    pub ctx: ExtensionContext,
    pub(crate) transport: Box<dyn ExtensionTransport>,
    pub(crate) tools: Vec<Tool>,
    pub(crate) commands: Vec<Command>,
    pub(crate) shortcuts: Vec<Shortcut>,
    pub(crate) flags: Vec<Flag>,
    pub(crate) providers: Vec<Provider>,
    pub(crate) mcp_servers: Vec<McpServer>,
}

impl Pi {
    /// Build the `pi` object (runtime entry; transports in WP-1205/1206
    /// call this, tests use [`NullTransport`]). Wires `ctx.ui` from the
    /// transport's UI surface.
    pub fn new(ctx: ExtensionContext, transport: Box<dyn ExtensionTransport>) -> Self {
        let mut ctx = ctx;
        ctx.ui = transport.ui();
        Self {
            ctx,
            transport,
            tools: Vec::new(),
            commands: Vec::new(),
            shortcuts: Vec::new(),
            flags: Vec::new(),
            providers: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }

    /// Register a tool the LLM can call.
    pub fn register_tool(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    /// The registered tools (in registration order).
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Register a custom command (upstream `registerCommand`); a
    /// same-name re-registration replaces it.
    pub fn register_command(&mut self, command: Command) {
        assert!(
            !command.name.is_empty(),
            "command must have a non-empty name"
        );
        self.commands.retain(|c| c.name != command.name);
        self.commands.push(command);
    }

    /// The registered commands (in registration order).
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Register a keyboard shortcut (upstream `registerShortcut`); the
    /// same key replaces the earlier registration.
    pub fn register_shortcut(&mut self, shortcut: Shortcut) {
        self.shortcuts.retain(|s| s.key != shortcut.key);
        self.shortcuts.push(shortcut);
    }

    /// The registered shortcuts (in registration order).
    pub fn shortcuts(&self) -> &[Shortcut] {
        &self.shortcuts
    }

    /// Register a CLI flag (upstream `registerFlag`); a same-name
    /// re-registration replaces it.
    pub fn register_flag(&mut self, flag: Flag) {
        self.flags.retain(|f| f.name != flag.name);
        self.flags.push(flag);
    }

    /// The registered flags (in registration order).
    pub fn flags(&self) -> &[Flag] {
        &self.flags
    }

    /// `pi.getFlag` — the CLI value when given on the process command
    /// line (the core spawns the extension with the CLI flags), else
    /// the registered default; `None` when the flag is not registered.
    /// Upstream `applyExtensionFlagValues` coercion: a **boolean**
    /// flag is `true` for any CLI occurrence (an `=value` is ignored);
    /// a **string** flag takes the CLI string value — a bare occurrence
    /// (no value) carries none, so the default stands.
    #[must_use]
    pub fn get_flag(&self, name: &str) -> Option<Value> {
        let flag = self.flags.iter().find(|f| f.name == name)?;
        resolve_flag_value(flag.kind, cli_flag_value(name), flag.default.clone())
    }

    /// Register or override a model provider (upstream
    /// `registerProvider`); a same-name re-registration replaces it.
    pub fn register_provider(&mut self, provider: Provider) {
        self.providers.retain(|p| p.name != provider.name);
        self.providers.push(provider);
    }

    /// `pi.unregisterProvider` — removes the named provider (no-op when
    /// not registered).
    pub fn unregister_provider(&mut self, name: &str) {
        self.providers.retain(|p| p.name != name);
    }

    /// The registered providers (in registration order).
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    /// Register an MCP server (upstream `registerMcpServer` — the core
    /// hosts it); a same-name re-registration replaces the
    /// extension's earlier registration.
    pub fn register_mcp_server(&mut self, server: McpServer) {
        self.mcp_servers.retain(|s| s.name != server.name);
        self.mcp_servers.push(server);
    }

    /// The registered MCP servers (in registration order).
    pub fn mcp_servers(&self) -> &[McpServer] {
        &self.mcp_servers
    }

    /// The context of this extension instance.
    pub fn ctx(&self) -> &ExtensionContext {
        &self.ctx
    }

    /// `pi.sendUserMessage` — always triggers a turn.
    pub fn send_user_message(
        &self,
        content: MessageContent,
        options: Option<SendUserMessageOptions>,
    ) {
        self.transport.send_user_message(content, options);
    }

    /// `pi.appendEntry` — custom entry for state persistence (not sent
    /// to the LLM).
    pub fn append_entry(&self, custom_type: &str, data: Value) {
        self.transport.append_entry(custom_type, data);
    }

    /// `pi.setSessionName`.
    pub fn set_session_name(&self, name: &str) {
        self.transport.set_session_name(name);
    }

    /// `pi.getSessionName`.
    pub fn session_name(&self) -> Option<String> {
        self.ctx.session_name.clone()
    }

    /// `pi.setLabel` — `None` clears.
    pub fn set_label(&self, entry_id: &str, label: Option<&str>) {
        self.transport.set_label(entry_id, label);
    }

    /// `pi.getSettings` — a copy of the effective settings as JSON.
    pub fn get_settings(&self) -> Value {
        self.transport.get_settings()
    }

    /// `pi.setThinkingLevel`.
    pub fn set_thinking_level(&self, level: &str) {
        self.transport.set_thinking_level(level);
    }

    /// `pi.getThinkingLevel`.
    pub fn thinking_level(&self) -> Option<String> {
        self.ctx.thinking_level.clone()
    }

    /// `pi.setModel` — `true` when the switch happened (false when the
    /// provider has no auth).
    pub async fn set_model(&self, model: ModelRef) -> bool {
        self.transport.set_model(model).await
    }

    /// `pi.exec` — run a shell command.
    pub async fn exec(&self, command: &str, args: &[&str]) -> ExecResult {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.transport.exec(command, &args).await
    }
}

/// The value of `--<name>` on the process command line, upstream
/// `parseArgs`' `unknownFlags` semantics: `--name=value`, `--name
/// value` (the following non-dash/non-`@` arg is the value) or a bare
/// `--name` = `true`; the scan **stops at `--`** (everything after is
/// positional) and the **last occurrence wins** (the `unknownFlags`
/// map is last-write-wins); `None` when absent.
fn cli_flag_value(name: &str) -> Option<Value> {
    let args: Vec<String> = std::env::args().collect();
    cli_flag_value_from_args(&args, name)
}

/// [`cli_flag_value`] over an explicit argv (the parse is pure so the
/// upstream semantics are testable without process args).
fn cli_flag_value_from_args(args: &[String], name: &str) -> Option<Value> {
    let mut value: Option<Value> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            break; // everything after `--` is positional
        }
        let Some(rest) = arg.strip_prefix("--") else {
            i += 1;
            continue;
        };
        let mut next_i = i + 1;
        if let Some(eq) = rest.find('=') {
            if &rest[..eq] == name {
                value = Some(Value::String(rest[eq + 1..].to_string()));
            }
        } else if rest == name {
            // Upstream: a bare `--name` takes the next arg as its value
            // unless it starts with `-` or `@` (then it is `true`); the
            // value arg is consumed.
            if args
                .get(next_i)
                .is_some_and(|n| !n.starts_with('-') && !n.starts_with('@'))
            {
                value = Some(Value::String(args[next_i].clone()));
                next_i += 1;
            } else {
                value = Some(Value::Bool(true));
            }
        }
        i = next_i;
    }
    value
}

/// Upstream `applyExtensionFlagValues` + `getFlag` resolution: a
/// **boolean** flag is `true` for any CLI occurrence (an `=value` is
/// ignored); a **string** flag takes the CLI string value — a bare
/// occurrence (upstream: "requires a value" error) or no occurrence
/// leaves the registered default standing.
fn resolve_flag_value(kind: FlagKind, cli: Option<Value>, default: Option<Value>) -> Option<Value> {
    match kind {
        FlagKind::Boolean => cli.is_some().then_some(Value::Bool(true)).or(default),
        FlagKind::String => match cli {
            Some(v) if v.is_string() => Some(v),
            _ => default,
        },
    }
}

/// The extension itself: the init entry (the Rust analog of upstream's
/// `export default (pi) => { … }`) plus one handler per v6-catalog event.
///
/// Fire-and-forget events have unit-returning handlers; decision events
/// return `Option<…>` — `None` (the default) means "no change / default
/// resolution", `Some(…)` the handler's result.
pub trait Extension: Send {
    /// Called once with the `pi` object before any event; register
    /// tools here.
    fn init(&mut self, _pi: &mut Pi) {}

    // ------------------------------------------------------------------
    // Fire-and-forget events
    // ------------------------------------------------------------------

    fn on_session_start(&self, _event: &SessionStartEvent, _ctx: &ExtensionContext) {}
    fn on_session_info_changed(&self, _event: &SessionInfoChanged, _ctx: &ExtensionContext) {}
    fn on_session_compact(&self, _event: &SessionCompactEvent, _ctx: &ExtensionContext) {}
    fn on_session_compact_failed(
        &self,
        _event: &SessionCompactFailedEvent,
        _ctx: &ExtensionContext,
    ) {
    }
    fn on_session_shutdown(&self, _event: &SessionShutdownEvent, _ctx: &ExtensionContext) {}
    fn on_agent_start(&self, _event: &crate::wire::AgentStartEvent, _ctx: &ExtensionContext) {}
    fn on_agent_end(&self, _event: &crate::wire::AgentEndEvent, _ctx: &ExtensionContext) {}
    fn on_turn_start(&self, _event: &TurnStartEvent, _ctx: &ExtensionContext) {}
    fn on_turn_end(&self, _event: &TurnEndEvent, _ctx: &ExtensionContext) {}
    fn on_message_start(&self, _event: &MessageStartEvent, _ctx: &ExtensionContext) {}
    fn on_message_update(&self, _event: &MessageUpdateEvent, _ctx: &ExtensionContext) {}
    fn on_message_end(&self, _event: &MessageEndEvent, _ctx: &ExtensionContext) {}
    fn on_model_select(&self, _event: &ModelSelectEvent, _ctx: &ExtensionContext) {}
    fn on_thinking_level_select(&self, _event: &ThinkingLevelSelectEvent, _ctx: &ExtensionContext) {
    }
    /// The core's MCP set changed (configured + extension-registered).
    fn on_mcp_servers_change(&self, _event: &McpServersChangeEvent, _ctx: &ExtensionContext) {}

    // ------------------------------------------------------------------
    // Decision events
    // ------------------------------------------------------------------

    /// Block / annotate the prompt before a run starts.
    fn on_before_agent_start(
        &self,
        _event: &BeforeAgentStartEvent,
        _ctx: &ExtensionContext,
    ) -> Option<BeforeAgentStartResult> {
        None
    }
    /// Filter the conversation (no system messages; v0 core answers
    /// `null`).
    fn on_context(
        &self,
        _event: &ContextEvent,
        _ctx: &ExtensionContext,
    ) -> Option<Vec<AgentMessage>> {
        None
    }
    /// Filter the full transcript (the result is used as-is).
    fn on_context_with_system(
        &self,
        _event: &ContextWithSystemEvent,
        _ctx: &ExtensionContext,
    ) -> Option<Vec<AgentMessage>> {
        None
    }
    /// Block or mutate a tool call's input before execution.
    fn on_tool_call(
        &self,
        _event: &ToolCallEvent,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallEventResult> {
        None
    }
    /// Mutate a tool result after execution.
    fn on_tool_result(
        &self,
        _event: &ToolResultEvent,
        _ctx: &ExtensionContext,
    ) -> Option<ToolResultEventResult> {
        None
    }
    /// Transform / handle user input before agent processing.
    fn on_input(&self, _event: &InputEvent, _ctx: &ExtensionContext) -> Option<InputEventResult> {
        None
    }
    /// Handle a user-run bash command.
    fn on_user_bash(
        &self,
        _event: &UserBashEvent,
        _ctx: &ExtensionContext,
    ) -> Option<UserBashEventResult> {
        None
    }
    /// Cancel compaction or supply the summary.
    fn on_session_before_compact(
        &self,
        _event: &SessionBeforeCompactEvent,
        _ctx: &ExtensionContext,
    ) -> Option<SessionBeforeCompactResult> {
        None
    }
}

// Re-exported for doc-example ergonomics (`pi_ext::pi::SessionStartEvent`).
pub use crate::wire::{
    AgentEndEvent, AgentMessage, AgentStartEvent, AgentToolResult, AssistantMessageEvent,
    BeforeAgentStartEvent, BeforeAgentStartResult, ContentBlock, ContextEvent,
    ContextWithSystemEvent, InputEvent, InputEventResult, McpServerEntry, McpServersChangeEvent,
    MessageContent, MessageEndEvent, MessageStartEvent, MessageUpdateEvent, ModelRef,
    ModelSelectEvent, SessionBeforeCompactEvent, SessionBeforeCompactResult, SessionCompactEvent,
    SessionCompactFailedEvent, SessionInfoChanged, SessionShutdownEvent, SessionStartEvent,
    ThinkingLevelSelectEvent, ToolCallEvent, ToolCallEventResult, ToolResultEvent,
    ToolResultEventResult, TurnEndEvent, TurnStartEvent, UserBashEvent, UserBashEventResult,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::McpServerConfig;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Records the calls, for asserting the `Pi` → transport wiring.
    #[derive(Clone, Default)]
    struct Recording {
        user_messages: Arc<Mutex<Vec<String>>>,
        entries: Arc<Mutex<Vec<(String, Value)>>>,
        names: Arc<Mutex<Vec<String>>>,
        labels: Arc<Mutex<Vec<LabelRec>>>,
    }

    type LabelRec = (String, Option<String>);

    #[derive(Default)]
    struct RecordingTransport(Recording);

    impl ExtensionTransport for RecordingTransport {
        fn send_user_message(
            &self,
            content: MessageContent,
            _options: Option<SendUserMessageOptions>,
        ) {
            let text = match content {
                MessageContent::Text(t) => t,
                MessageContent::Blocks(_) => "<blocks>".into(),
            };
            self.0.user_messages.lock().unwrap().push(text);
        }
        fn append_entry(&self, custom_type: &str, data: Value) {
            self.0
                .entries
                .lock()
                .unwrap()
                .push((custom_type.to_string(), data));
        }
        fn set_session_name(&self, name: &str) {
            self.0.names.lock().unwrap().push(name.to_string());
        }
        fn set_label(&self, entry_id: &str, label: Option<&str>) {
            self.0
                .labels
                .lock()
                .unwrap()
                .push((entry_id.to_string(), label.map(str::to_string)));
        }
        fn get_settings(&self) -> Value {
            json!({"theme": "dark"})
        }
        fn set_thinking_level(&self, _level: &str) {}
        fn set_model(&self, _model: ModelRef) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { true })
        }
        fn exec(
            &self,
            command: &str,
            _args: &[String],
        ) -> Pin<Box<dyn Future<Output = ExecResult> + Send + '_>> {
            let command = command.to_string();
            Box::pin(async move {
                ExecResult {
                    stdout: command,
                    stderr: String::new(),
                    code: 0,
                    killed: false,
                    truncated: false,
                }
            })
        }
        fn execute_tool(
            &self,
            name: &str,
            _args: Value,
        ) -> Pin<Box<dyn Future<Output = Result<AgentToolResult, String>> + Send + '_>> {
            let name = name.to_string();
            Box::pin(async move { Ok(AgentToolResult::text(name)) })
        }
        fn ui(&self) -> Arc<dyn ExtensionUI> {
            Arc::new(NullUi)
        }
    }

    fn recording_pi() -> (Pi, Recording) {
        let rec = Recording::default();
        let pi = Pi::new(
            ExtensionContext::default(),
            Box::new(RecordingTransport(rec.clone())),
        );
        (pi, rec)
    }

    #[test]
    fn pi_routes_actions_to_the_transport() {
        let (pi, rec) = recording_pi();
        pi.send_user_message(MessageContent::Text("hi".into()), None);
        pi.append_entry("state", json!({"n": 1}));
        pi.set_session_name("My session");
        pi.set_label("e1", Some("bookmark"));
        pi.set_label("e1", None);
        assert_eq!(pi.get_settings(), json!({"theme": "dark"}));

        let rec = &rec;
        assert_eq!(
            rec.user_messages.lock().unwrap().as_slice(),
            &["hi".to_string()]
        );
        assert_eq!(
            rec.entries.lock().unwrap().as_slice(),
            &[("state".to_string(), json!({"n": 1}))]
        );
        assert_eq!(
            rec.names.lock().unwrap().as_slice(),
            &["My session".to_string()]
        );
        let labels = rec.labels.lock().unwrap();
        assert_eq!(labels[0], ("e1".to_string(), Some("bookmark".to_string())));
        assert_eq!(labels[1], ("e1".to_string(), None));
    }

    #[tokio::test]
    async fn pi_async_actions() {
        let (pi, _rec) = recording_pi();
        let m = ModelRef {
            provider: "fake".into(),
            id: "fake/m".into(),
            api: None,
            name: None,
        };
        assert!(pi.set_model(m).await);
        let r = pi.exec("echo", &["hello"]).await;
        assert_eq!(r.stdout, "echo");
        assert_eq!(r.code, 0);
    }

    /// No overrides: decision defaults are `None`, F&F handlers no-op.
    struct Bare;
    impl Extension for Bare {}

    #[test]
    fn handler_defaults() {
        let ext = Bare;
        let ctx = ExtensionContext::default();
        let tc = crate::wire::ToolCallEvent {
            kind: "tool_call".into(),
            tool_call_id: "t1".into(),
            parent_tool_call_id: None,
            tool_name: "bash".into(),
            input: json!({}),
        };
        assert_eq!(ext.on_tool_call(&tc, &ctx), None);
        assert_eq!(
            ext.on_input(
                &crate::wire::InputEvent {
                    kind: "input".into(),
                    text: "x".into(),
                    images: None,
                    source: None,
                    streaming_behavior: None,
                },
                &ctx,
            ),
            None
        );
    }

    struct Blocking {
        seen: Arc<AtomicBool>,
    }
    impl Extension for Blocking {
        fn on_tool_call(
            &self,
            event: &ToolCallEvent,
            _ctx: &ExtensionContext,
        ) -> Option<ToolCallEventResult> {
            self.seen.store(true, Ordering::SeqCst);
            (event.tool_name == "write").then_some(ToolCallEventResult {
                block: Some(true),
                reason: Some("gated".into()),
                terminate: None,
            })
        }
        fn on_session_start(&self, _event: &SessionStartEvent, _ctx: &ExtensionContext) {
            self.seen.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn handler_overrides_dispatch() {
        let seen = Arc::new(AtomicBool::new(false));
        let ext = Blocking {
            seen: Arc::clone(&seen),
        };
        let ctx = ExtensionContext::default();

        let tc = ToolCallEvent {
            kind: "tool_call".into(),
            tool_call_id: "t1".into(),
            parent_tool_call_id: None,
            tool_name: "write".into(),
            input: json!({}),
        };
        let r = ext.on_tool_call(&tc, &ctx).unwrap();
        assert_eq!(r.block, Some(true));
        assert_eq!(r.reason.as_deref(), Some("gated"));

        let other = ToolCallEvent {
            tool_name: "read".into(),
            ..tc
        };
        assert_eq!(ext.on_tool_call(&other, &ctx), None);

        ext.on_session_start(
            &SessionStartEvent {
                kind: "session_start".into(),
                reason: None,
                previous_session_file: None,
            },
            &ctx,
        );
        assert!(seen.load(Ordering::SeqCst));
    }

    fn command(name: &str) -> Command {
        Command::new(name, |_args: String, _ctx: &ExtensionContext| {
            Box::pin(async { None })
        })
    }

    #[test]
    fn command_registration_replaces_same_name() {
        let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        pi.register_command(command("gates"));
        pi.register_command(command("gates"));
        pi.register_command(command("other"));
        assert_eq!(pi.commands().len(), 2);
        assert_eq!(pi.commands()[0].name, "gates");
        assert_eq!(pi.commands()[1].name, "other");
    }

    #[test]
    #[should_panic(expected = "non-empty name")]
    fn empty_command_name_is_rejected() {
        let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        pi.register_command(command(""));
    }

    #[test]
    fn shortcut_registration_replaces_same_key() {
        let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        pi.register_shortcut(Shortcut::new("ctrl+g", |_ctx: &ExtensionContext| {
            Box::pin(async {})
        }));
        pi.register_shortcut(Shortcut::new("ctrl+g", |_ctx: &ExtensionContext| {
            Box::pin(async {})
        }));
        pi.register_shortcut(Shortcut::new("ctrl+h", |_ctx: &ExtensionContext| {
            Box::pin(async {})
        }));
        assert_eq!(pi.shortcuts().len(), 2);
        assert_eq!(pi.shortcuts()[0].key, "ctrl+g");
        assert_eq!(pi.shortcuts()[1].key, "ctrl+h");
    }

    #[test]
    fn get_flag_resolves_default_for_unspecified_flag() {
        let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        pi.register_flag(Flag::new("gate-mode", FlagKind::String).with_default(json!("warn")));
        pi.register_flag(Flag::new("loud", FlagKind::Boolean));
        // No --gate-mode/--loud on this process's command line.
        assert_eq!(pi.get_flag("gate-mode"), Some(json!("warn")));
        assert_eq!(pi.get_flag("loud"), None);
        assert_eq!(pi.get_flag("nope"), None);
        // Same-name re-registration replaces.
        pi.register_flag(Flag::new("gate-mode", FlagKind::String).with_default(json!("block")));
        assert_eq!(pi.flags().len(), 2);
        assert_eq!(pi.get_flag("gate-mode"), Some(json!("block")));
    }

    #[test]
    #[should_panic(expected = "Invalid default for flag")]
    fn flag_default_type_mismatch_is_rejected() {
        let _ = Flag::new("gate-mode", FlagKind::String).with_default(json!(true));
    }

    /// Upstream `parseArgs` `unknownFlags` semantics, pinned: the scan
    /// stops at `--` and the last occurrence wins.
    #[test]
    fn cli_flag_value_upstream_semantics() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // `--name=value`, incl. an empty value (still an occurrence).
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet=world"]), "greet"),
            Some(json!("world"))
        );
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet="]), "greet"),
            Some(json!(""))
        );
        // `--name value` (the value arg is consumed); a following
        // dash/`@` arg is not the value → bare occurrence = `true`.
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet", "world"]), "greet"),
            Some(json!("world"))
        );
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet", "--other"]), "greet"),
            Some(json!(true))
        );
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet", "@file"]), "greet"),
            Some(json!(true))
        );
        // The last occurrence wins (upstream last-write-wins).
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet=first", "--greet=second"]), "greet"),
            Some(json!("second"))
        );
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet=first", "--greet", "second"]), "greet"),
            Some(json!("second"))
        );
        // The scan stops at `--`: everything after is positional.
        assert_eq!(
            cli_flag_value_from_args(&a(&["--", "--greet=world"]), "greet"),
            None
        );
        assert_eq!(
            cli_flag_value_from_args(&a(&["--greet=first", "--", "--greet=second"]), "greet"),
            Some(json!("first"))
        );
        // Other flags' values are not occurrences of `greet`.
        assert_eq!(
            cli_flag_value_from_args(&a(&["--other", "world"]), "greet"),
            None
        );
    }

    /// Upstream `applyExtensionFlagValues` coercion, pinned: a boolean
    /// flag is `true` for any occurrence (`=x` ignored), a string flag
    /// only for a valued one (a bare occurrence leaves the default
    /// standing).
    #[test]
    fn get_flag_coerces_by_flag_kind() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Boolean: `--loud=x` and a bare `--loud` are both `true`.
        assert_eq!(
            resolve_flag_value(
                FlagKind::Boolean,
                cli_flag_value_from_args(&a(&["--loud=x"]), "loud"),
                None,
            ),
            Some(json!(true))
        );
        assert_eq!(
            resolve_flag_value(
                FlagKind::Boolean,
                cli_flag_value_from_args(&a(&["--loud"]), "loud"),
                None,
            ),
            Some(json!(true))
        );
        // No occurrence: the registered default stands (or `None`).
        assert_eq!(
            resolve_flag_value(FlagKind::Boolean, None, Some(json!(true))),
            Some(json!(true))
        );
        assert_eq!(resolve_flag_value(FlagKind::Boolean, None, None), None);
        // String: the CLI value is taken as-is…
        assert_eq!(
            resolve_flag_value(
                FlagKind::String,
                cli_flag_value_from_args(&a(&["--greet", "world"]), "greet"),
                Some(json!("default")),
            ),
            Some(json!("world"))
        );
        // …a bare occurrence carries no value → the default stands
        // (upstream: "requires a value" error, no flagValue set).
        assert_eq!(
            resolve_flag_value(
                FlagKind::String,
                cli_flag_value_from_args(&a(&["--greet"]), "greet"),
                Some(json!("default")),
            ),
            Some(json!("default"))
        );
        // No occurrence and no default: `None`.
        assert_eq!(resolve_flag_value(FlagKind::String, None, None), None);
    }

    fn provider_config() -> ProviderConfig {
        ProviderConfig::new(
            vec![ProviderModel::new("faketest/echo-1").with_api("faketest-api")],
            |_model, _messages, _options, _emit| Box::pin(async {}),
        )
    }

    #[test]
    fn provider_and_mcp_registration_replace_same_name() {
        let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        pi.register_provider(Provider::new("faketest", provider_config()));
        pi.register_provider(Provider::new("faketest", provider_config()));
        assert_eq!(pi.providers().len(), 1);
        assert_eq!(pi.providers()[0].name, "faketest");
        assert_eq!(
            pi.providers()[0].config.models[0].api.as_deref(),
            Some("faketest-api")
        );
        pi.unregister_provider("faketest");
        pi.unregister_provider("faketest"); // no-op
        assert!(pi.providers().is_empty());

        pi.register_mcp_server(McpServer::new("docs", McpServerConfig::default()));
        pi.register_mcp_server(McpServer::new("docs", McpServerConfig::default()));
        assert_eq!(pi.mcp_servers().len(), 1);
        assert_eq!(pi.mcp_servers()[0].name, "docs");
    }

    #[tokio::test]
    async fn ctx_ui_is_the_transport_ui() {
        // NullTransport carries the NullUi no-UI default.
        let pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
        let ctx = pi.ctx();
        let opts = UiDialogOptions::default();
        assert_eq!(ctx.ui.select("t", &["a".into()], &opts).await, None);
        assert!(!ctx.ui.confirm("t", "m", &opts).await);
        assert_eq!(ctx.ui.input("t", None, &opts).await, None);
        assert_eq!(ctx.ui.editor("t", None).await, None);
        assert_eq!(
            ctx.ui
                .custom(Box::new(|_p: Pane| Box::pin(async {})), None)
                .await,
            None
        );
    }
}
