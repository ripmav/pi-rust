//! Example user extension (WP-1205): a bridge-v6-speaking process.
//! Registers tools (dialog + custom pane), commands, event handlers,
//! a flag, a shortcut, a provider and an MCP server, and is driven over
//! NDJSON stdio by `pi-ext/tests/process.rs` — the same contract the
//! core has for a TS extension (spawn, `init`, `ready`, dispatch).

use pi_ext::pi::{
    Command, Extension, ExtensionContext, Flag, FlagKind, McpServer, Pane, PaneDescriptor, Pi,
    Provider, ProviderConfig, ProviderModel, Shortcut, Tool, UiDialogOptions,
};
use pi_ext::wire::{
    AgentMessage, AgentToolResult, AssistantMessage, AssistantMessageEvent, ContentBlock,
    McpServerConfig, MessageContent, PaneLine, PartialMessage, ToolCallEvent, ToolCallEventResult,
};
use serde_json::{json, Value};

struct Demo;

impl Extension for Demo {
    fn events(&self) -> Vec<&'static str> {
        vec!["session_start", "session_shutdown", "tool_call"]
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
        // Tool: pane — a custom pane (v6 `custom`): one descriptor
        // update, then `done` with the key that ended it. The optional
        // `mode` input drives the failure/edge shapes the harness pins:
        // `panic` (factory panics before its first host call),
        // `panic_late` (panics after the update), `timeout` (never
        // done — the capability timeout closes the pane) and `hang`
        // (done, then the component keeps awaiting — the runtime must
        // not wait for it).
        pi.register_tool(Tool::new(
            "pane",
            "Open a custom pane",
            pi_ext::pi::schema::object(
                &[(
                    "mode",
                    pi_ext::pi::schema::string(Some(
                        "demo mode: panic | panic_late | timeout | hang (default: happy path)",
                    )),
                )],
                &[],
            ),
            |input, ctx| {
                Box::pin(async move {
                    let mode = input
                        .get("mode")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let result = ctx
                        .ctx()
                        .ui
                        .custom(
                            Box::new(move |pane: Pane| {
                                Box::pin(async move {
                                    if mode.as_deref() == Some("panic") {
                                        panic!("demo pane factory panic");
                                    }
                                    pane.update(&PaneDescriptor {
                                        title: Some("Demo pane".into()),
                                        lines: Some(vec![PaneLine {
                                            text: "press a key".into(),
                                        }]),
                                        ..Default::default()
                                    });
                                    match mode.as_deref() {
                                        Some("panic_late") => {
                                            panic!("demo pane factory panic (late)")
                                        }
                                        Some("timeout") => {
                                            // Never done: the capability
                                            // timeout must close the pane.
                                            let _ = pane.next_key().await;
                                        }
                                        Some("hang") => {
                                            // `done` is the pane's end
                                            // signal; the component keeps
                                            // awaiting afterwards.
                                            pane.done(json!("done"));
                                            let _ = pane.next_key().await;
                                        }
                                        _ => {
                                            let key = pane.next_key().await;
                                            pane.done(json!(key.map(|k| k.code)));
                                        }
                                    }
                                })
                            }),
                            None,
                        )
                        .await;
                    let text = match result {
                        Some(Value::String(s)) => format!("pane: {s}"),
                        Some(_) => "pane: done".into(),
                        None => "pane: dismissed".into(),
                    };
                    Ok(AgentToolResult::text(text))
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
        // Shortcut: Ctrl+D — registered in mixed case on purpose: the
        // dispatch must match case-insensitively (upstream keys the
        // shortcut map by the lowercased key) while `ready.shortcuts`
        // keeps the registered case. A `shortcut` line with
        // `key: "ctrl+d"` must reach this handler.
        pi.register_shortcut(Shortcut::new("Ctrl+D", |_ctx: &ExtensionContext| {
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

    /// The `session_shutdown` handler: panics on `reason: "panic"`
    /// (the panic-safety check — the process must still exit 0).
    fn on_session_shutdown(
        &self,
        event: &pi_ext::pi::SessionShutdownEvent,
        _ctx: &ExtensionContext,
    ) {
        if event.reason.as_deref() == Some("panic") {
            panic!("demo session_shutdown panic");
        }
        eprintln!("[demo] session shutdown: {:?}", event.reason);
    }
}

fn main() {
    pi_ext::process::run(Demo);
}
