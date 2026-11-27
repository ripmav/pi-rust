//! Command, shortcut and flag registration — upstream
//! `registerCommand` / `registerShortcut` / `registerFlag` / `getFlag`
//! (the wire-expressible subset per ADR 0002): the
//! `ready.commands`/`ready.shortcuts`/`ready.flags` snapshot plus the
//! `command`/`shortcut` dispatch lines (v4). Flag *values* never cross
//! the wire — the core spawns the extension with the CLI flags, and
//! `getFlag` resolves them from the process command line (upstream CLI
//! parsing: `--name=value`, `--name value`, bare `--name` = `true`),
//! falling back to the registered default.

use crate::pi::ExtensionContext;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// A command handler: the raw args + the extension context → the
/// handler's return value for the `command` response (`None` = JSON
/// `null`, `Some(v)` = the value).
pub type CommandHandler = Box<
    dyn Fn(String, &ExtensionContext) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + '_>>
        + Send
        + Sync,
>;

/// A registered command (upstream `RegisteredCommand` minus
/// `sourceInfo` and the non-wire `getArgumentCompletions`).
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub handler: CommandHandler,
}

impl Command {
    /// Build a command.
    #[must_use]
    pub fn new<H>(name: impl Into<String>, handler: H) -> Self
    where
        H: Fn(
                String,
                &ExtensionContext,
            ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + '_>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            name: name.into(),
            description: None,
            handler: Box::new(handler),
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A shortcut handler (upstream: `(ctx) => void | Promise<void>`).
pub type ShortcutHandler =
    Box<dyn Fn(&ExtensionContext) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> + Send + Sync>;

/// A registered keyboard shortcut (upstream `ExtensionShortcut` minus
/// `extensionPath`, which the transport fills in at `ready`).
pub struct Shortcut {
    pub key: String,
    pub description: Option<String>,
    pub handler: ShortcutHandler,
}

impl Shortcut {
    /// Build a shortcut for a key id (e.g. `"ctrl+g"`).
    #[must_use]
    pub fn new<H>(key: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&ExtensionContext) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            key: key.into(),
            description: None,
            handler: Box::new(handler),
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A flag's type (upstream `type: "boolean" | "string"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    Boolean,
    String,
}

impl FlagKind {
    /// The wire value (`ready.flags[].type`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::String => "string",
        }
    }
}

/// A registered CLI flag (upstream `ExtensionFlag` minus
/// `extensionPath`, which the transport fills in at `ready`).
#[derive(Debug, Clone)]
pub struct Flag {
    pub name: String,
    pub description: Option<String>,
    pub kind: FlagKind,
    pub default: Option<Value>,
}

impl Flag {
    /// Build a flag.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: FlagKind) -> Self {
        Self {
            name: name.into(),
            description: None,
            kind,
            default: None,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the default (checked against `kind` — upstream
    /// validation; panics on a mismatch).
    #[must_use]
    pub fn with_default(mut self, default: Value) -> Self {
        let matches = match self.kind {
            FlagKind::Boolean => default.is_boolean(),
            FlagKind::String => default.is_string(),
        };
        assert!(
            matches,
            "Invalid default for flag \"{}\": expected {}, got {}",
            self.name,
            self.kind.as_str(),
            json_value_type_name(&default),
        );
        self.default = Some(default);
        self
    }
}

/// The upstream `typeof` of a JSON value (error messages).
fn json_value_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
