//! Provider and MCP registration — upstream `registerProvider` /
//! `registerMcpServer` (the wire-expressible subset per ADR 0002): the
//! `ready.providers`/`providers_changed` snapshot (v1), the
//! `provider_stream` invocation, and the `mcp_server_register` forward
//! (v6). The provider object itself stays with the extension — only the
//! model list crosses the wire, and the core invokes `stream` per
//! `provider_stream`.

use crate::wire::{
    AgentMessage, AssistantMessageEvent, McpServerConfig, ModelRef, ProviderStreamOptions,
};
use std::future::Future;
use std::pin::Pin;

/// A `streamSimple` stand-in: one `provider_stream` invocation. `model`
/// is the selected model (`ready.providers` entry), `messages` the
/// conversation, `options` the sampling options. Call `emit` once per
/// upstream `AssistantMessageEvent`; the terminal `done`/`error` event
/// ends the stream.
pub type ProviderStreamFn = Box<
    dyn for<'a> Fn(
            ModelRef,
            Vec<AgentMessage>,
            &ProviderStreamOptions,
            &'a mut dyn FnMut(AssistantMessageEvent),
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
        + Send
        + Sync,
>;

/// A provider model (the `ready.providers[].models` subset; the wire
/// `provider` field is the provider's `name`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderModel {
    pub id: String,
    pub api: Option<String>,
    pub name: Option<String>,
}

impl ProviderModel {
    /// Build a model.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            api: None,
            name: None,
        }
    }

    /// Set the API type.
    #[must_use]
    pub fn with_api(mut self, api: impl Into<String>) -> Self {
        self.api = Some(api.into());
        self
    }

    /// Set the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Provider config (upstream `ProviderConfig`, the wire-expressible
/// subset): the model list + the stream handler. Provider-level
/// `baseUrl`/`apiKey`/`api` stay with the extension's own stream
/// implementation — they never cross the wire.
pub struct ProviderConfig {
    pub models: Vec<ProviderModel>,
    /// `streamSimple` — invoked by the `provider_stream` command.
    pub stream: ProviderStreamFn,
}

impl ProviderConfig {
    /// Build the config from its models + `streamSimple` closure
    /// (boxed internally).
    #[must_use]
    pub fn new<F>(models: Vec<ProviderModel>, stream: F) -> Self
    where
        F: for<'a> Fn(
                ModelRef,
                Vec<AgentMessage>,
                &ProviderStreamOptions,
                &'a mut dyn FnMut(AssistantMessageEvent),
            ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            models,
            stream: Box::new(stream),
        }
    }
}

/// A registered provider (upstream
/// `registerProvider(name, config)`).
pub struct Provider {
    pub name: String,
    pub config: ProviderConfig,
}

impl Provider {
    /// Build a provider.
    #[must_use]
    pub fn new(name: impl Into<String>, config: ProviderConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }
}

/// A registered MCP server (upstream `registerMcpServer`); `config` is
/// the `mcp.json` entry shape — the same type that crosses the wire in
/// `mcp_server_register` (the core hosts the server).
#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub config: McpServerConfig,
}

impl McpServer {
    /// Build a server registration.
    #[must_use]
    pub fn new(name: impl Into<String>, config: McpServerConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }
}
