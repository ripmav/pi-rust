//! The extension API core — the 1:1 Rust mirror of the upstream `pi`
//! object + `ctx` (ADR 0002), limited to what the bridge v6 catalog can
//! express: the [`Extension`] trait (init entry + event handlers), the
//! context ([`ExtensionContext`]), tool registration ([`Tool`],
//! [`schema`] helpers), and the message/session/model surface on
//! [`Pi`].
//!
//! UI, commands, shortcuts, flags, providers and MCP land in WP-1204;
//! the transports (v6 user process, core cdylib) in WP-1205/1206.
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
//! Runtime actions (messages, session, model) run through the
//! [`ExtensionTransport`] supplied by the runtime — [`NullTransport`]
//! no-ops them, which is what a standalone compile/test needs:
//!
//! ```
//! use pi_ext::pi::{ExtensionContext, NullTransport, Pi};
//! use pi_ext::wire::MessageContent;
//!
//! let pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
//! pi.send_user_message(MessageContent::Text("hi".into()), None);
//! pi.append_entry("my-state", serde_json::json!({"step": 1}));
//! ```

mod tool;

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
pub use tool::{schema, Tool, ToolContext, ToolExecutor, ToolOutcome};

/// The extension context (upstream `ExtensionContext`, the wire-
/// expressible subset per ADR 0002): the run mode, dialog availability,
/// working directory and the session/model view.
#[derive(Debug, Clone, Default, PartialEq)]
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
}

/// The `pi` object handed to [`Extension::init`] and used from handlers:
/// registrations collected during `init`, runtime actions through the
/// transport.
pub struct Pi {
    /// The context for this extension instance.
    pub ctx: ExtensionContext,
    pub(crate) transport: Box<dyn ExtensionTransport>,
    pub(crate) tools: Vec<Tool>,
}

impl Pi {
    /// Build the `pi` object (runtime entry; transports in WP-1205/1206
    /// call this, tests use [`NullTransport`]).
    pub fn new(ctx: ExtensionContext, transport: Box<dyn ExtensionTransport>) -> Self {
        Self {
            ctx,
            transport,
            tools: Vec::new(),
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
    ContextWithSystemEvent, InputEvent, InputEventResult, MessageContent, MessageEndEvent,
    MessageStartEvent, MessageUpdateEvent, ModelRef, ModelSelectEvent, SessionBeforeCompactEvent,
    SessionBeforeCompactResult, SessionCompactEvent, SessionCompactFailedEvent, SessionInfoChanged,
    SessionShutdownEvent, SessionStartEvent, ThinkingLevelSelectEvent, ToolCallEvent,
    ToolCallEventResult, ToolResultEvent, ToolResultEventResult, TurnEndEvent, TurnStartEvent,
    UserBashEvent, UserBashEventResult,
};

#[cfg(test)]
mod tests {
    use super::*;
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
}
