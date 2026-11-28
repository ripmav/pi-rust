//! Round-trip tests for the bridge v6 wire types (WP-1202).
//!
//! Fixtures: `fixtures/bridge-protocol-examples.jsonl` — the wire
//! examples from docs/bridge-protocol.md, verbatim, except the MCP
//! example's `session_start` line, where the framing-required `id` is
//! added (the doc abbreviates it; §Framing makes `id` mandatory on
//! command lines); `fixtures/catalog.jsonl` — synthetic lines covering
//! the rest of the message catalog.

use pi_ext::wire::*;
use serde_json::{json, Value};

fn lines(name: &str) -> Vec<String> {
    let path = format!("tests/fixtures/{name}");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {path}: {e}"))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The typed model cannot carry an explicit JSON `null` in a place where
/// the producer may simply omit the field (both mean "absent" on the
/// wire — e.g. `Response.data`), so such nulls are dropped on
/// re-serialization and stripped from the original before comparing.
/// Nulls inside map values are meaningful (`SystemMessage.sections`
/// removes a section with `null`) and never appear in the fixtures.
fn strip_nuls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            for x in m.values_mut() {
                strip_nuls(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nuls),
        _ => {}
    }
}

#[test]
fn every_fixture_line_roundtrips() {
    for name in ["bridge-protocol-examples.jsonl", "catalog.jsonl"] {
        for (i, line) in lines(name).iter().enumerate() {
            let wire: BridgeLine = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{name} line {i} does not parse: {e}\n{line}"));
            let rt = serde_json::to_string(&wire).expect("serialize");
            let mut orig: Value = serde_json::from_str(line).unwrap();
            strip_nuls(&mut orig);
            let rtv: Value = serde_json::from_str(&rt).unwrap();
            assert_eq!(
                rtv, orig,
                "{name} line {i} round-trip mismatch:\n{line}\nvs\n{rt}"
            );
        }
    }
}

#[test]
fn doc_examples_carry_the_right_types() {
    let ls = lines("bridge-protocol-examples.jsonl");
    let l = |n: usize| ls[n].as_str();

    // init: protocol 6 + headless capabilities + the docs MCP server.
    let init: BridgeLine = serde_json::from_str(l(0)).unwrap();
    match init {
        BridgeLine::Command(Command::Init {
            protocol_version,
            capabilities,
            mcp_config,
            project_trusted,
            ..
        }) => {
            assert_eq!(protocol_version, 6);
            assert!(capabilities.ui.is_empty());
            assert_eq!(capabilities.timeout_ms, 30000);
            let srv = &mcp_config.unwrap().mcp_servers["docs"];
            assert_eq!(srv.command.as_deref(), Some("docs-server"));
            assert_eq!(srv.exposure.as_deref(), Some("direct"));
            assert_eq!(project_trusted, None);
        }
        other => panic!("init line: {other:?}"),
    }

    // ready: the permission-gate registry snapshot.
    let ready: BridgeLine = serde_json::from_str(l(2)).unwrap();
    match ready {
        BridgeLine::Event(SidecarEvent::Ready(r)) => {
            assert_eq!(r.api_version, 0);
            assert_eq!(
                r.extensions[0].path,
                "/home/u/proj/.pi/extensions/permission-gate/index.ts"
            );
            assert_eq!(r.tools[0].name, "gate");
            assert_eq!(r.tools[0].label.as_deref(), Some("Gate"));
            assert_eq!(r.flags[0].default, Some(Value::String("warn".into())));
            assert_eq!(r.providers[0].name, "faketest");
            assert_eq!(
                r.providers[0].models[0].api.as_deref(),
                Some("faketest-api")
            );
            assert_eq!(r.events, vec!["tool_call", "tool_result", "session_start"]);
            assert_eq!(r.shortcuts, None); // pre-v4 snapshot
        }
        other => panic!("ready line: {other:?}"),
    }

    // tool_call decision: blocked, input unchanged.
    let resp: BridgeLine = serde_json::from_str(l(12)).unwrap();
    match resp {
        BridgeLine::Response(Response {
            success,
            data: Some(data),
            ..
        }) if success => {
            let tc: ToolCallResponse = serde_json::from_value(data).unwrap();
            assert_eq!(tc.result.as_ref().unwrap().block, Some(true));
            assert_eq!(
                tc.result.as_ref().unwrap().reason.as_deref(),
                Some("path is gated")
            );
            assert_eq!(tc.input["path"], Value::String("etc/passwd".into()));
        }
        other => panic!("tool_call response: {other:?}"),
    }

    // provider stream terminal event: done with the final message.
    let pse: BridgeLine = serde_json::from_str(l(7)).unwrap();
    match pse {
        BridgeLine::Event(SidecarEvent::ProviderStreamEvent { stream_id, event }) => {
            assert_eq!(stream_id, "s-42");
            match event {
                AssistantMessageEvent::Done { reason, message } => {
                    assert_eq!(reason, "stop");
                    match message {
                        AgentMessage::Assistant(a) => {
                            assert_eq!(a.usage.as_ref().unwrap().input, Some(3));
                            assert_eq!(a.stop_reason.as_deref(), Some("stop"));
                        }
                        other => panic!("final message: {other:?}"),
                    }
                }
                other => panic!("done event: {other:?}"),
            }
        }
        other => panic!("provider_stream_event line: {other:?}"),
    }

    // dialog request: confirm.
    let ui: BridgeLine = serde_json::from_str(l(15)).unwrap();
    match ui {
        BridgeLine::Event(SidecarEvent::ExtensionUiRequest(UiRequest::Confirm {
            id,
            title,
            message,
            timeout,
        })) => {
            assert_eq!(id, "d7");
            assert_eq!(title, "Proceed?");
            assert_eq!(message, "This rewrites 3 files");
            assert_eq!(timeout, Some(30000));
        }
        other => panic!("ui request line: {other:?}"),
    }

    // a handler threw: the in-flight decision still resolves to default.
    let err: BridgeLine = serde_json::from_str(l(17)).unwrap();
    match err {
        BridgeLine::Event(SidecarEvent::ExtensionError(e)) => {
            assert_eq!(e.event, "tool_call");
            assert_eq!(
                e.extension_path,
                "/home/u/proj/.pi/extensions/broken/index.ts"
            );
        }
        other => panic!("extension_error line: {other:?}"),
    }
}

#[test]
fn catalog_lines_carry_the_right_types() {
    let ls = lines("catalog.jsonl");
    let l = |n: usize| ls[n].as_str();

    // before_agent_start decision: custom message + replaced prompt.
    let resp: BridgeLine = serde_json::from_str(l(2)).unwrap();
    match resp {
        BridgeLine::Response(Response {
            data: Some(data), ..
        }) => {
            let r: BeforeAgentStartResult = serde_json::from_value(data).unwrap();
            assert_eq!(r.messages.as_ref().unwrap().len(), 1);
            assert_eq!(r.system_prompt.as_deref(), Some("You are pi (gated)."));
        }
        other => panic!("before_agent_start response: {other:?}"),
    }

    // context_with_system: the full transcript incl. system + toolResult.
    let cws: BridgeLine = serde_json::from_str(l(3)).unwrap();
    match cws {
        BridgeLine::Command(Command::ContextWithSystem { event, .. }) => {
            let msgs = &event.messages;
            assert_eq!(msgs.len(), 4);
            assert!(matches!(&msgs[0], AgentMessage::System(_)));
            assert!(matches!(&msgs[3], AgentMessage::ToolResult(m) if !m.is_error));
            if let AgentMessage::Assistant(a) = &msgs[2] {
                assert_eq!(
                    a.usage.as_ref().unwrap().cost.as_ref().unwrap().total,
                    Some(Value::from(0.3))
                );
            } else {
                panic!("assistant message");
            }
        }
        other => panic!("context_with_system: {other:?}"),
    }

    // input decision: transform.
    let input: BridgeLine = serde_json::from_str(l(6)).unwrap();
    match input {
        BridgeLine::Response(Response {
            data: Some(data), ..
        }) => {
            let r: InputEventResult = serde_json::from_value(data).unwrap();
            assert!(matches!(
                r,
                InputEventResult::Transform { ref text, .. } if text == "do it (logged in as u)"
            ));
        }
        other => panic!("input response: {other:?}"),
    }

    // session_before_compact: extension-supplied summary.
    let sbc: BridgeLine = serde_json::from_str(l(19)).unwrap();
    match sbc {
        BridgeLine::Response(Response {
            data: Some(data), ..
        }) => {
            let r: SessionBeforeCompactResult = serde_json::from_value(data).unwrap();
            assert_eq!(
                r.compaction.as_ref().unwrap().summary.as_deref(),
                Some("old stuff")
            );
        }
        other => panic!("session_before_compact response: {other:?}"),
    }

    // session_compact: the persisted compaction entry.
    let sc: BridgeLine = serde_json::from_str(l(21)).unwrap();
    match sc {
        BridgeLine::Command(Command::SessionCompact { event, .. }) => {
            assert_eq!(event.compaction_entry.from_hook, Some(true));
            assert_eq!(
                event.compaction_entry.first_kept_entry_id.as_deref(),
                Some("e5")
            );
        }
        other => panic!("session_compact: {other:?}"),
    }

    // v5 ready: shortcuts + diagnostics + activeTools.
    let ready: BridgeLine = serde_json::from_str(l(38)).unwrap();
    match ready {
        BridgeLine::Event(SidecarEvent::Ready(r)) => {
            assert_eq!(r.shortcuts.as_ref().unwrap()[0].key, "ctrl+g");
            assert_eq!(r.shortcut_diagnostics.as_ref().unwrap().len(), 1);
            assert_eq!(r.active_tools.as_ref(), Some(&Vec::new()));
        }
        other => panic!("v5 ready: {other:?}"),
    }

    // the parse-error response: id is null on the wire.
    let parse_err: BridgeLine = serde_json::from_str(l(40)).unwrap();
    match parse_err {
        BridgeLine::Response(Response {
            id,
            command,
            success,
            error,
            ..
        }) => {
            assert_eq!(id, None);
            assert_eq!(command, "parse");
            assert!(!success);
            assert!(error.is_some());
        }
        other => panic!("parse error response: {other:?}"),
    }

    // v6 pane key: the id is the pane id.
    let pk: BridgeLine = serde_json::from_str(l(52)).unwrap();
    match pk {
        BridgeLine::Command(Command::PaneKey { id, key }) => {
            assert_eq!(id, "p1");
            assert_eq!(key.code, "down");
        }
        other => panic!("pane_key: {other:?}"),
    }

    // v6 MCP forward (E3S5 follow-up): the extension's registerMcpServer.
    let mreg: BridgeLine = serde_json::from_str(l(54)).unwrap();
    match mreg {
        BridgeLine::Event(SidecarEvent::McpServerRegister {
            name,
            config,
            extension_path,
        }) => {
            assert_eq!(name, "jira");
            assert_eq!(config.url.as_deref(), Some("https://mcp.example.com/jira"));
            assert_eq!(config.exposure.as_deref(), Some("direct"));
            assert_eq!(extension_path, "/home/u/.pi/extensions/jira/index.ts");
        }
        other => panic!("mcp_server_register: {other:?}"),
    }

    // The core's full MCP set (configured + extension-registered).
    let mchange: BridgeLine = serde_json::from_str(l(56)).unwrap();
    match mchange {
        BridgeLine::Event(SidecarEvent::McpServersChange { event }) => {
            assert_eq!(event.servers.len(), 2);
            assert_eq!(event.servers[0].name, "cfg");
            assert_eq!(event.servers[0].extension_path, None);
            assert_eq!(event.servers[1].name, "jira");
            assert_eq!(
                event.servers[1].extension_path.as_deref(),
                Some("/home/u/.pi/extensions/jira/index.ts")
            );
        }
        other => panic!("mcp_servers_change: {other:?}"),
    }
}

#[test]
fn system_sections_null_is_meaningful_and_preserved() {
    // A `null` section value removes the section — it must survive the
    // round-trip (map values are not null-dropped like Option fields).
    let line = r#"{"role":"system","content":"base","sections":{"gate":null,"keep":"stay"}}"#;
    let m: AgentMessage = serde_json::from_str(line).unwrap();
    match m {
        AgentMessage::System(ref s) => {
            let sections = s.sections.as_ref().unwrap();
            assert_eq!(sections.get("gate"), Some(&None));
            assert_eq!(sections.get("keep"), Some(&Some("stay".into())));
            let rt = serde_json::to_string(&m).unwrap();
            assert!(rt.contains(r#""gate":null"#), "null section dropped: {rt}");
        }
        other => panic!("system message: {other:?}"),
    }
}

/// A full upstream-shaped `mcp.json` entry (the set the core's
/// `register_server` path accepts) round-trips through
/// [`McpServerConfig`] with the exact camelCase field names the core
/// reads (`src/mcp.rs` `parse_servers`/`parse_stdio`/`parse_http`).
#[test]
fn mcp_server_config_full_entry_roundtrips() {
    // stdio: every field the core accepts.
    let stdio: Value = json!({
        "type": "stdio",
        "command": "node",
        "args": ["server.js", "--port", "3000"],
        "cwd": "/home/u/proj",
        "env": { "TOKEN": "${API_TOKEN}" },
        "exposure": "direct",
        "toolExposure": { "search": "deferred", "get*": "hidden" },
        "timeout": 90.5,
        "enabled": true
    });
    let cfg: McpServerConfig = serde_json::from_value(stdio.clone()).unwrap();
    assert_eq!(cfg.server_type.as_deref(), Some("stdio"));
    assert_eq!(cfg.command.as_deref(), Some("node"));
    let want_args = vec![
        "server.js".to_string(),
        "--port".to_string(),
        "3000".to_string(),
    ];
    assert_eq!(cfg.args.as_ref(), Some(&want_args));
    assert_eq!(cfg.cwd.as_deref(), Some("/home/u/proj"));
    assert_eq!(
        cfg.env.as_ref().unwrap().get("TOKEN").map(String::as_str),
        Some("${API_TOKEN}")
    );
    assert_eq!(cfg.exposure.as_deref(), Some("direct"));
    assert_eq!(cfg.tool_exposure.as_ref().unwrap()["get*"], "hidden");
    assert_eq!(cfg.timeout, Some(90.5));
    assert_eq!(cfg.enabled, Some(true));
    // Re-serialization keeps the core's expected camelCase names.
    assert_eq!(serde_json::to_value(&cfg).unwrap(), stdio);

    // http: incl. the opaque `auth` passthrough (the core rejects it,
    // D17 — the wire still carries it).
    let http: Value = json!({
        "type": "http",
        "url": "https://mcp.example.com/jira",
        "headers": { "Authorization": "Bearer ${TOKEN}" },
        "exposure": "codemode",
        "timeout": 45.5,
        "enabled": false,
        "auth": { "provider": "faketest" }
    });
    let cfg: McpServerConfig = serde_json::from_value(http.clone()).unwrap();
    assert_eq!(cfg.server_type.as_deref(), Some("http"));
    assert_eq!(cfg.url.as_deref(), Some("https://mcp.example.com/jira"));
    assert_eq!(
        cfg.headers
            .as_ref()
            .unwrap()
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer ${TOKEN}")
    );
    assert_eq!(cfg.enabled, Some(false));
    assert_eq!(cfg.auth.as_ref().unwrap()["provider"], "faketest");
    assert_eq!(serde_json::to_value(&cfg).unwrap(), http);

    // Whole-second timeouts (the common case) parse too; the core
    // rounds to whole seconds.
    let int_timeout: McpServerConfig =
        serde_json::from_value(json!({ "type": "stdio", "command": "node", "timeout": 60 }))
            .unwrap();
    assert_eq!(int_timeout.timeout, Some(60.0));
}
