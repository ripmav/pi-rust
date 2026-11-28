//! Example user extension (WP-1205): a bridge-v6-speaking process.
//! Registers a tool + command + event handlers + flag + shortcut +
//! provider + MCP server and is driven over NDJSON stdio by
//! `pi-ext/tests/process.rs` — the same contract the core has for a TS
//! extension (spawn, `init`, `ready`, dispatch).

use pi_ext::pi::{
    Command, Extension, ExtensionContext, Flag, FlagKind, McpServer, Pi, Provider, ProviderConfig,
    ProviderModel, Shortcut, Tool, UiDialogOptions,
};
use pi_ext::wire::{
    AgentMessage, AgentToolResult, AssistantMessage, AssistantMessageEvent, ContentBlock,
    McpServerConfig, MessageContent, PartialMessage, ToolCallEvent, ToolCallEventResult,
};
use serde_json::{json, Value};

struct Demo;

impl Extension for Demo {
    fn events(&self) -> Vec<&'static str> {
        vec!["session_start", "tool_call"]
    }

    fn init(&mut self, pi: &mut Pi) {
        // Tool: hello — the `extension_tool_call` text round-trip.
        pi.register_tool(pi_ext::pi::Tool::new(
            "hello",
            "Say hello to someone",
            pi_ext::pi::schema::object(
                &[("name", pi_ext::pi::schema::string(Some("Who to greet")))],
                &["name"],
            ),
            |input, _ctx| {
                Box::pin(async move {
                    let name = input["name"].as_str().unwrap_or("world");
                    Ok(AgentToolResult::text(format!("Hello, {name}!")))
                })
            },
        ));
        // Tool: ask — opens a confirm dialog (the UI round-trip).
        pi.register_tool(Tool::new(
            "ask",
            "Ask the user for confirmation",
            pi_ext::pi::schema::object(&[], &[]),
            |_input, ctx| {
                Box::pin(async move {
                    let ok = ctx
                        .ctx()
                        .ui
                        .confirm(
                            "Proceed?",
                            "This is a demo dialog",
                            &UiDialogOptions::default(),
                        )
                        .await;
                    Ok(AgentToolResult::text(if ok {
                        "confirmed"
                    } else {
                        "declined"
                    }))
                })
            },
        ));
        // Command: greet — echoes its args (the `command` round-trip).
        pi.register_command(
            Command::new("greet", |args: String, _ctx: &ExtensionContext| {
                Box::pin(async move { Some(json!({ "args": args })) })
            })
            .with_description("Greet with args"),
        );
        // Command: boom — panics (the panic-safety check).
        pi.register_command(Command::new(
            "boom",
            |_args: String, _ctx: &ExtensionContext| {
                Box::pin(async move { panic!("demo command panic") })
            },
        ));
        // Flag: greet — resolved from the process command line
        // (`--greet=…`), else the registered default.
        pi.register_flag(Flag::new("greet", FlagKind::String).with_default(json!("world")));
        eprintln!(
            "[demo] greet flag = {}",
            pi.get_flag("greet").unwrap_or(Value::Null)
        );
        // Shortcut: ctrl+d — a ready.shortcuts entry (the `shortcut`
        // dispatch line).
        pi.register_shortcut(Shortcut::new("ctrl+d", |_ctx: &ExtensionContext| {
            Box::pin(async {})
        }));
        // Provider: faketest — a scripted one-line echo stream
        // (`provider_stream` + `provider_stream_event`).
        pi.register_provider(Provider::new(
            "faketest",
            ProviderConfig::new(
                vec![ProviderModel::new("echo-1").with_api("faketest-api")],
                |_model, messages, _options, emit| {
                    Box::pin(async move {
                        let echo = messages
                            .iter()
                            .rev()
                            .find_map(|m| match m {
                                AgentMessage::User(u) => match &u.content {
                                    MessageContent::Text(t) => Some(t.clone()),
                                    _ => None,
                                },
                                _ => None,
                            })
                            .unwrap_or_default();
                        let text = format!("echo: {echo}");
                        emit(AssistantMessageEvent::Start {
                            partial: PartialMessage::Sparse(
                                json!({ "role": "assistant", "content": [] }),
                            ),
                        });
                        emit(AssistantMessageEvent::TextDelta {
                            content_index: 0,
                            delta: text.clone(),
                            partial: PartialMessage::Sparse(json!({})),
                        });
                        emit(AssistantMessageEvent::Done {
                            reason: "stop".into(),
                            message: AgentMessage::Assistant(AssistantMessage {
                                content: vec![ContentBlock::Text {
                                    text,
                                    text_signature: None,
                                }],
                                stop_reason: Some("stop".into()),
                                ..Default::default()
                            }),
                        });
                    })
                },
            ),
        ));
        // MCP server: forwarded to the core as `mcp_server_register`
        // (v6, additive) right after init.
        pi.register_mcp_server(McpServer::new(
            "docs",
            McpServerConfig {
                command: Some("docs-server".into()),
                ..Default::default()
            },
        ));
    }

    /// The `tool_call` decision: block calls flagged `dangerous`
    /// (permission-gate style) and panic on `panic: true` (the
    /// panic-safety check).
    fn on_tool_call(
        &self,
        event: &ToolCallEvent,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallEventResult> {
        if event.input.get("panic") == Some(&json!(true)) {
            panic!("demo tool_call panic")
        }
        (event.input.get("dangerous") == Some(&json!(true))).then_some(ToolCallEventResult {
            block: Some(true),
            reason: Some("dangerous call blocked by demo".into()),
            terminate: None,
        })
    }

    fn on_session_start(&self, _event: &pi_ext::pi::SessionStartEvent, _ctx: &ExtensionContext) {
        eprintln!("[demo] session started");
    }
}

fn main() {
    pi_ext::process::run(Demo);
}
