//! Tool registration (upstream `ToolDefinition` / `AgentTool`, the wire-
//! expressible subset per ADR 0002): [`Tool`] with a JSON-schema
//! `parameters` and an async `execute`, plus the [`schema`] helpers for
//! writing the schema without a TypeBox equivalent.

use crate::pi::{ExtensionContext, ExtensionTransport};
use crate::wire::{AgentToolResult, ContentBlock};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// A tool's outcome: `Ok` = the [`AgentToolResult`] (set `is_error` for
/// tool failures the model should see), `Err` = an error result
/// (`isError: true`, the message as text content).
pub type ToolOutcome = Result<AgentToolResult, String>;

/// A tool's `execute` closure: input (the model's arguments, already
/// validated against `parameters`) + the [`ToolContext`].
pub type ToolExecutor = Box<
    dyn for<'a> Fn(
            Value,
            &'a ToolContext<'a>,
        ) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>
        + Send
        + Sync,
>;

/// A registered tool (upstream `ToolDefinition`, minus the sidecar-only
/// fields — renderers, prepare hooks, reach rules — that the v6 catalog
/// cannot express).
pub struct Tool {
    /// Tool name (used in LLM tool calls).
    pub name: String,
    /// Human-readable label for UI.
    pub label: Option<String>,
    /// Description for the LLM.
    pub description: Option<String>,
    /// One-line snippet for the Available-tools system-prompt section.
    pub prompt_snippet: Option<String>,
    /// Parameter schema (JSON Schema object).
    pub parameters: Value,
    /// JSON Schema of `structuredContent` in successful results.
    pub output_schema: Option<Value>,
    /// How the model reaches the tool: `"direct"` (default) |
    /// `"model-only"` | `"codemode"` | `"deferred"`.
    pub exposure: Option<String>,
    /// Whether registering the tool activates it (default `true` for
    /// `direct`/`model-only`).
    pub default_active: Option<bool>,
    /// Execute the tool.
    pub execute: ToolExecutor,
}

impl Tool {
    /// Build a tool from its required parts. `execute` is any
    /// `Fn(Value, &ToolContext) -> future` closure (boxed internally).
    #[must_use]
    pub fn new<E>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        execute: E,
    ) -> Self
    where
        E: for<'a> Fn(
                Value,
                &'a ToolContext<'a>,
            ) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            name: name.into(),
            label: None,
            description: Some(description.into()),
            prompt_snippet: None,
            parameters,
            output_schema: None,
            exposure: None,
            default_active: None,
            execute: Box::new(execute),
        }
    }

    /// Set the UI label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the system-prompt snippet.
    #[must_use]
    pub fn with_prompt_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.prompt_snippet = Some(snippet.into());
        self
    }

    /// Set the exposure.
    #[must_use]
    pub fn with_exposure(mut self, exposure: impl Into<String>) -> Self {
        self.exposure = Some(exposure.into());
        self
    }

    /// Set whether registration activates the tool.
    #[must_use]
    pub fn with_default_active(mut self, default_active: bool) -> Self {
        self.default_active = Some(default_active);
        self
    }

    /// Set the `structuredContent` output schema.
    #[must_use]
    pub fn with_output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }
}

/// Context passed to a tool's `execute` (upstream
/// `ExtensionToolContext`, the wire-expressible subset).
pub struct ToolContext<'a> {
    pub(crate) ctx: &'a ExtensionContext,
    pub(crate) transport: &'a dyn ExtensionTransport,
}

impl<'a> ToolContext<'a> {
    /// Build the context (runtime entry; tests).
    #[must_use]
    pub fn new(ctx: &'a ExtensionContext, transport: &'a dyn ExtensionTransport) -> Self {
        Self { ctx, transport }
    }

    /// The extension context of the running extension.
    pub fn ctx(&self) -> &ExtensionContext {
        self.ctx
    }

    /// Run another tool through the core's normal hooks (validation,
    /// `tool_call`/`tool_result` events, permissions). Never fails the
    /// host: failures come back as `Err`.
    pub fn execute_tool(
        &self,
        name: &str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + '_>> {
        self.transport.execute_tool(name, args)
    }
}

/// A plain text result (the common case).
#[must_use]
fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![ContentBlock::Text {
            text: text.into(),
            text_signature: None,
        }],
        details: None,
        structured_content: None,
        usage: None,
        is_error: None,
        terminate: None,
    }
}

impl AgentToolResult {
    /// A plain text result (the common case).
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        text_result(text)
    }

    /// A failure result the model sees (`isError: true`).
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        let mut r = Self::text(text);
        r.is_error = Some(true);
        r
    }
}

/// JSON Schema helpers for `Tool.parameters` — the Rust stand-in for
/// TypeBox, kept deliberately small.
pub mod schema {
    use serde_json::{json, Value};

    /// An object schema: `{"type":"object","properties":…,"required":…}`.
    /// `props` are (name, sub-schema) pairs in order.
    #[must_use]
    pub fn object(props: &[(&str, Value)], required: &[&str]) -> Value {
        let mut properties = serde_json::Map::new();
        for (k, v) in props {
            properties.insert((*k).to_string(), v.clone());
        }
        let mut v = json!({"type": "object", "properties": properties});
        if !required.is_empty() {
            v["required"] = Value::Array(required.iter().map(|s| json!(s)).collect());
        }
        v
    }

    /// `{"type":"string","description":…?}`.
    #[must_use]
    pub fn string(description: Option<&str>) -> Value {
        typed("string", description)
    }

    /// `{"type":"integer","description":…?}`.
    #[must_use]
    pub fn integer(description: Option<&str>) -> Value {
        typed("integer", description)
    }

    /// `{"type":"number","description":…?}`.
    #[must_use]
    pub fn number(description: Option<&str>) -> Value {
        typed("number", description)
    }

    /// `{"type":"boolean","description":…?}`.
    #[must_use]
    pub fn boolean(description: Option<&str>) -> Value {
        typed("boolean", description)
    }

    /// `{"type":"array","items":…,"description":…?}`.
    #[must_use]
    pub fn array(items: Value, description: Option<&str>) -> Value {
        let mut v = typed("array", description);
        v["items"] = items;
        v
    }

    fn typed(ty: &str, description: Option<&str>) -> Value {
        let mut v = json!({"type": ty});
        if let Some(d) = description {
            v["description"] = json!(d);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::NullTransport;
    use serde_json::json;

    #[test]
    fn schema_object_shape() {
        let s = schema::object(
            &[
                ("path", schema::string(Some("The file"))),
                ("force", schema::boolean(None)),
            ],
            &["path"],
        );
        assert_eq!(
            s,
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The file"},
                    "force": {"type": "boolean"}
                },
                "required": ["path"]
            })
        );
    }

    #[test]
    fn schema_object_without_required_omits_key() {
        let s = schema::object(&[("a", schema::integer(Some("N")))], &[]);
        assert_eq!(
            s["properties"]["a"],
            json!({"type": "integer", "description": "N"})
        );
        assert!(s.get("required").is_none());
    }

    #[test]
    fn schema_type_helpers() {
        assert_eq!(schema::number(None), json!({"type": "number"}));
        assert_eq!(
            schema::array(json!({"type": "string"}), Some("A list")),
            json!({"type": "array", "description": "A list", "items": {"type": "string"}})
        );
    }

    #[test]
    fn tool_new_and_builders() {
        let tool = Tool::new(
            "gate",
            "Allow or deny",
            json!({"type": "object"}),
            |_: Value, _ctx: &ToolContext| Box::pin(async { Ok(AgentToolResult::text("ok")) }),
        )
        .with_label("Gate")
        .with_prompt_snippet("Gate file paths")
        .with_exposure("model-only")
        .with_default_active(false)
        .with_output_schema(json!({"type": "object"}));
        assert_eq!(tool.name, "gate");
        assert_eq!(tool.label.as_deref(), Some("Gate"));
        assert_eq!(tool.description.as_deref(), Some("Allow or deny"));
        assert_eq!(tool.prompt_snippet.as_deref(), Some("Gate file paths"));
        assert_eq!(tool.exposure.as_deref(), Some("model-only"));
        assert_eq!(tool.default_active, Some(false));
        assert!(tool.output_schema.is_some());
    }

    /// Runs the returned future on a current-thread runtime (dev-dep
    /// only; the test futures complete without I/O).
    fn block_on<F: std::future::Future + Send>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    #[test]
    fn tool_execute_runs_with_context() {
        let tool = Tool::new(
            "cwd-tool",
            "echo cwd",
            json!({"type": "object"}),
            |input: Value, ctx: &ToolContext| {
                Box::pin(async move {
                    let n = input["n"].as_u64().unwrap_or(0);
                    Ok(AgentToolResult::text(format!("{}-{}", ctx.ctx().cwd, n)))
                })
            },
        );
        let ctx = ExtensionContext {
            cwd: "/work".into(),
            ..Default::default()
        };
        let tc = ToolContext {
            ctx: &ctx,
            transport: &NullTransport,
        };
        let exec = &tool.execute;
        let r = block_on(exec(json!({"n": 2}), &tc)).expect("tool error");
        let text = r
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .unwrap();
        assert_eq!(text, "/work-2");
    }
}
