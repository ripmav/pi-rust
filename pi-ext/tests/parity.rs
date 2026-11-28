//! Catalog ↔ API parity (the ADR 0002 parity rule, WP-1204).
//!
//! `fixtures/v6-catalog.jsonl` is the maintained single source of truth
//! for the bridge v6 capability catalog — docs/bridge-protocol.md
//! (message catalog, `init` capabilities, the `ready` registry and the
//! local-access capabilities a v6 extension may exercise). This test
//! diffs it against the API surface *exercised* by [`ParityProbe`]:
//! every registration, event handler, action and UI method the probe
//! touches records its capability id, so the API set is built from live
//! API calls, not a hardcoded list.
//!
//! - a catalog capability with no API item fails the forward diff;
//! - an API item the probe exercises that the catalog does not carry
//!   fails the reverse diff (add it to the fixture only if the protocol
//!   gained it);
//! - deleting an API surface item breaks the probe (compile) or drops
//!   its recorded capability (diff failure).
//!
//! When the bridge protocol changes: update the fixture here and the
//! human-readable source in docs/bridge-protocol.md together.

use pi_ext::pi::*;
use pi_ext::wire::{
    CompactPreparation, CompactionEntry, McpServerConfig, PartialMessage, UserMessage,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<&'static str>>>);

impl Recorder {
    fn record(&self, cap: &'static str) {
        self.0.lock().unwrap().push(cap);
    }
}

/// Exercises the whole API surface once, recording each capability it
/// touches.
struct ParityProbe {
    rec: Recorder,
}

impl Extension for ParityProbe {
    fn init(&mut self, pi: &mut Pi) {
        let rec = &self.rec;
        rec.record("register:tool");
        pi.register_tool(Tool::new(
            "parity",
            "Parity probe tool",
            json!({"type": "object"}),
            |_: Value, _ctx: &ToolContext| Box::pin(async { Ok(AgentToolResult::text("parity")) }),
        ));
        rec.record("register:command");
        pi.register_command(
            Command::new("parity", |_args: String, _ctx: &ExtensionContext| {
                Box::pin(async { None })
            })
            .with_description("Parity probe command"),
        );
        rec.record("register:shortcut");
        pi.register_shortcut(Shortcut::new("ctrl+parity", |_ctx: &ExtensionContext| {
            Box::pin(async {})
        }));
        rec.record("register:flag");
        pi.register_flag(Flag::new("parity", FlagKind::String).with_default(json!("probe")));
        rec.record("action:get_flag");
        let _ = pi.get_flag("parity");
        rec.record("register:provider");
        pi.register_provider(Provider::new(
            "parity",
            ProviderConfig::new(
                vec![ProviderModel::new("parity/probe")],
                |_model, _messages, _options, _emit| Box::pin(async {}),
            ),
        ));
        rec.record("action:unregister_provider");
        pi.unregister_provider("parity");
        rec.record("register:mcp-server");
        pi.register_mcp_server(McpServer::new("parity", McpServerConfig::default()));
    }

    fn on_session_start(&self, _e: &SessionStartEvent, _c: &ExtensionContext) {
        self.rec.record("event:session_start");
    }
    fn on_before_agent_start(
        &self,
        _e: &BeforeAgentStartEvent,
        _c: &ExtensionContext,
    ) -> Option<BeforeAgentStartResult> {
        self.rec.record("event:before_agent_start");
        None
    }
    fn on_agent_start(&self, _e: &AgentStartEvent, _c: &ExtensionContext) {
        self.rec.record("event:agent_start");
    }
    fn on_agent_end(&self, _e: &AgentEndEvent, _c: &ExtensionContext) {
        self.rec.record("event:agent_end");
    }
    fn on_turn_start(&self, _e: &TurnStartEvent, _c: &ExtensionContext) {
        self.rec.record("event:turn_start");
    }
    fn on_turn_end(&self, _e: &TurnEndEvent, _c: &ExtensionContext) {
        self.rec.record("event:turn_end");
    }
    fn on_message_start(&self, _e: &MessageStartEvent, _c: &ExtensionContext) {
        self.rec.record("event:message_start");
    }
    fn on_message_update(&self, _e: &MessageUpdateEvent, _c: &ExtensionContext) {
        self.rec.record("event:message_update");
    }
    fn on_message_end(&self, _e: &MessageEndEvent, _c: &ExtensionContext) {
        self.rec.record("event:message_end");
    }
    fn on_model_select(&self, _e: &ModelSelectEvent, _c: &ExtensionContext) {
        self.rec.record("event:model_select");
    }
    fn on_thinking_level_select(&self, _e: &ThinkingLevelSelectEvent, _c: &ExtensionContext) {
        self.rec.record("event:thinking_level_select");
    }
    fn on_context(&self, _e: &ContextEvent, _c: &ExtensionContext) -> Option<Vec<AgentMessage>> {
        self.rec.record("event:context");
        None
    }
    fn on_context_with_system(
        &self,
        _e: &ContextWithSystemEvent,
        _c: &ExtensionContext,
    ) -> Option<Vec<AgentMessage>> {
        self.rec.record("event:context_with_system");
        None
    }
    fn on_tool_call(
        &self,
        _e: &ToolCallEvent,
        _c: &ExtensionContext,
    ) -> Option<ToolCallEventResult> {
        self.rec.record("event:tool_call");
        None
    }
    fn on_tool_result(
        &self,
        _e: &ToolResultEvent,
        _c: &ExtensionContext,
    ) -> Option<ToolResultEventResult> {
        self.rec.record("event:tool_result");
        None
    }
    fn on_input(&self, _e: &InputEvent, _c: &ExtensionContext) -> Option<InputEventResult> {
        self.rec.record("event:input");
        None
    }
    fn on_user_bash(
        &self,
        _e: &UserBashEvent,
        _c: &ExtensionContext,
    ) -> Option<UserBashEventResult> {
        self.rec.record("event:user_bash");
        None
    }
    fn on_session_before_compact(
        &self,
        _e: &SessionBeforeCompactEvent,
        _c: &ExtensionContext,
    ) -> Option<SessionBeforeCompactResult> {
        self.rec.record("event:session_before_compact");
        None
    }
    fn on_session_compact(&self, _e: &SessionCompactEvent, _c: &ExtensionContext) {
        self.rec.record("event:session_compact");
    }
    fn on_session_compact_failed(&self, _e: &SessionCompactFailedEvent, _c: &ExtensionContext) {
        self.rec.record("event:session_compact_failed");
    }
    fn on_session_shutdown(&self, _e: &SessionShutdownEvent, _c: &ExtensionContext) {
        self.rec.record("event:session_shutdown");
    }
    fn on_session_info_changed(&self, _e: &SessionInfoChanged, _c: &ExtensionContext) {
        self.rec.record("event:session_info_changed");
    }
    fn on_mcp_servers_change(&self, _e: &McpServersChangeEvent, _c: &ExtensionContext) {
        self.rec.record("event:mcp_servers_change");
    }
}

/// The v6 catalog (fixture): `cap` per line, `ref` its basis in
/// docs/bridge-protocol.md.
fn fixture_caps() -> (BTreeSet<String>, usize) {
    let raw =
        std::fs::read_to_string("tests/fixtures/v6-catalog.jsonl").expect("reading the fixture");
    let mut caps = BTreeSet::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).expect("v6-catalog.jsonl line is not JSON");
        let cap = v["cap"]
            .as_str()
            .expect("v6-catalog.jsonl line needs a string `cap`");
        assert!(
            !v["ref"].as_str().unwrap_or_default().is_empty(),
            "capability {cap} has no `ref`"
        );
        assert!(
            caps.insert(cap.to_string()),
            "duplicate catalog capability {cap}"
        );
    }
    let n = caps.len();
    (caps, n)
}

#[tokio::test]
async fn catalog_and_api_parity() {
    let rec = Recorder::default();
    let mut probe = ParityProbe { rec: rec.clone() };

    let mut pi = Pi::new(ExtensionContext::default(), Box::new(NullTransport));
    probe.init(&mut pi);
    let ctx = pi.ctx();

    // Runtime actions (the WP-1203 surface).
    rec.record("action:send_user_message");
    pi.send_user_message(MessageContent::Text("parity".into()), None);
    rec.record("action:append_entry");
    pi.append_entry("parity", json!({}));
    rec.record("action:set_session_name");
    pi.set_session_name("parity");
    rec.record("action:get_session_name");
    let _ = pi.session_name();
    rec.record("action:set_label");
    pi.set_label("e1", Some("parity"));
    rec.record("action:get_settings");
    let _ = pi.get_settings();
    rec.record("action:set_thinking_level");
    pi.set_thinking_level("low");
    rec.record("action:get_thinking_level");
    let _ = pi.thinking_level();
    rec.record("action:set_model");
    let _ = pi
        .set_model(ModelRef {
            provider: "parity".into(),
            id: "parity/probe".into(),
            api: None,
            name: None,
        })
        .await;
    rec.record("action:exec");
    let _ = pi.exec("true", &[]).await;
    rec.record("action:execute_tool");
    let tc = ToolContext::new(ctx, &NullTransport);
    let _ = tc.execute_tool("parity", json!({})).await;

    // The UI surface (ctx.ui) + hasUI.
    let opts = UiDialogOptions::default();
    rec.record("ui:select");
    let _ = ctx
        .ui
        .select("parity", &["a".into(), "b".into()], &opts)
        .await;
    rec.record("ui:confirm");
    let _ = ctx.ui.confirm("parity", "parity?", &opts).await;
    rec.record("ui:input");
    let _ = ctx.ui.input("parity", Some("parity"), &opts).await;
    rec.record("ui:editor");
    let _ = ctx.ui.editor("parity", Some("x")).await;
    rec.record("ui:notify");
    ctx.ui.notify("parity", Some(NotifyKind::Info));
    rec.record("ui:custom");
    let _ = ctx
        .ui
        .custom(Box::new(|_pane: Pane| Box::pin(async {})), None)
        .await;
    rec.record("ui:set_status");
    ctx.ui.set_status("parity", Some("parity"));
    rec.record("ui:set_title");
    ctx.ui.set_title("parity");
    rec.record("ui:set_editor_text");
    ctx.ui.set_editor_text("parity");
    rec.record("ctx:has_ui");
    let _ = ctx.has_ui;

    // Every catalog event, one dummy payload each.
    let msg = AgentMessage::User(UserMessage {
        content: MessageContent::Text("parity".into()),
        timestamp: None,
    });
    let model = ModelRef {
        provider: "parity".into(),
        id: "parity/probe".into(),
        api: None,
        name: None,
    };
    probe.on_session_start(
        &SessionStartEvent {
            kind: "session_start".into(),
            reason: None,
            previous_session_file: None,
        },
        ctx,
    );
    probe.on_before_agent_start(
        &BeforeAgentStartEvent {
            kind: "before_agent_start".into(),
            message_type: None,
            prompt: None,
            images: None,
            message: None,
        },
        ctx,
    );
    probe.on_agent_start(
        &AgentStartEvent {
            kind: "agent_start".into(),
        },
        ctx,
    );
    probe.on_agent_end(
        &AgentEndEvent {
            kind: "agent_end".into(),
            messages: vec![],
            will_retry: None,
        },
        ctx,
    );
    probe.on_turn_start(
        &TurnStartEvent {
            kind: "turn_start".into(),
            turn_index: None,
            timestamp: None,
        },
        ctx,
    );
    probe.on_turn_end(
        &TurnEndEvent {
            kind: "turn_end".into(),
            turn_index: None,
            message: None,
            tool_results: None,
            message_entry_id: None,
            tool_result_entry_ids: None,
        },
        ctx,
    );
    probe.on_message_start(
        &MessageStartEvent {
            kind: "message_start".into(),
            message: msg.clone(),
        },
        ctx,
    );
    probe.on_message_update(
        &MessageUpdateEvent {
            kind: "message_update".into(),
            message: msg.clone(),
            assistant_message_event: AssistantMessageEvent::Start {
                partial: PartialMessage::Sparse(json!({})),
            },
        },
        ctx,
    );
    probe.on_message_end(
        &MessageEndEvent {
            kind: "message_end".into(),
            message: msg,
        },
        ctx,
    );
    probe.on_model_select(
        &ModelSelectEvent {
            kind: "model_select".into(),
            model,
            previous_model: None,
            source: None,
        },
        ctx,
    );
    probe.on_thinking_level_select(
        &ThinkingLevelSelectEvent {
            kind: "thinking_level_select".into(),
            level: "low".into(),
            previous_level: None,
        },
        ctx,
    );
    probe.on_tool_call(
        &ToolCallEvent {
            kind: "tool_call".into(),
            tool_call_id: "t1".into(),
            parent_tool_call_id: None,
            tool_name: "bash".into(),
            input: json!({}),
        },
        ctx,
    );
    probe.on_tool_result(
        &ToolResultEvent {
            kind: "tool_result".into(),
            tool_call_id: "t1".into(),
            parent_tool_call_id: None,
            tool_name: "bash".into(),
            input: json!({}),
            content: vec![],
            structured_content: None,
            is_error: false,
            usage: None,
            details: None,
        },
        ctx,
    );
    probe.on_context(
        &ContextEvent {
            kind: "context".into(),
            messages: vec![],
        },
        ctx,
    );
    probe.on_context_with_system(
        &ContextWithSystemEvent {
            kind: "context_with_system".into(),
            messages: vec![],
        },
        ctx,
    );
    probe.on_input(
        &InputEvent {
            kind: "input".into(),
            text: "parity".into(),
            images: None,
            source: None,
            streaming_behavior: None,
        },
        ctx,
    );
    probe.on_user_bash(
        &UserBashEvent {
            kind: "user_bash".into(),
            command: "ls".into(),
            exclude_from_context: None,
            cwd: None,
        },
        ctx,
    );
    probe.on_session_before_compact(
        &SessionBeforeCompactEvent {
            kind: "session_before_compact".into(),
            preparation: CompactPreparation::default(),
            branch_entries: vec![],
            custom_instructions: None,
            reason: None,
            will_retry: None,
        },
        ctx,
    );
    probe.on_session_compact(
        &SessionCompactEvent {
            kind: "session_compact".into(),
            compaction_entry: CompactionEntry::default(),
            from_extension: None,
            reason: None,
            will_retry: None,
        },
        ctx,
    );
    probe.on_session_compact_failed(
        &SessionCompactFailedEvent {
            kind: "session_compact_failed".into(),
            reason: None,
            error_message: None,
            aborted: None,
            will_retry: None,
            from_extension: None,
        },
        ctx,
    );
    probe.on_session_shutdown(
        &SessionShutdownEvent {
            kind: "session_shutdown".into(),
            reason: None,
            target_session_file: None,
        },
        ctx,
    );
    probe.on_session_info_changed(&SessionInfoChanged { name: None }, ctx);
    probe.on_mcp_servers_change(
        &McpServersChangeEvent {
            kind: "mcp_servers_change".into(),
            servers: vec![McpServerEntry {
                name: "parity".into(),
                config: McpServerConfig::default(),
                extension_path: None,
            }],
        },
        ctx,
    );

    let api: BTreeSet<String> = rec
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let (catalog, n) = fixture_caps();
    assert!(n > 0, "empty v6 catalog fixture");

    let missing_in_api: Vec<&String> = catalog.difference(&api).collect();
    let extra_in_api: Vec<&String> = api.difference(&catalog).collect();
    assert!(
        missing_in_api.is_empty(),
        "bridge v6 catalog capabilities missing from the pi-ext API:\n{missing_in_api:?}\n(add the API surface, or fix the fixture if the catalog changed — docs/bridge-protocol.md)"
    );
    assert!(
        extra_in_api.is_empty(),
        "pi-ext API capabilities missing from the bridge v6 catalog:\n{extra_in_api:?}\n(remove the API surface, or add it to the fixture only if the protocol gained it)"
    );
}
