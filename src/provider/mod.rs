//! Real model provider drivers (OpenAI-compatible, Anthropic).
//! The `ModelDriver` seam keeps the agent loop provider-agnostic; the bridge
//! and sidecar never see provider details. Credentials come from the caller
//! (env vars at the CLI layer) — never from config files.
pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicDriver;
pub use openai::OpenAiDriver;

use crate::model::{AssistantTurn, ModelDriver};
use anyhow::{Context, Result};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A live provider driver shared by the agent loop (cloned per run).
#[derive(Clone, Debug)]
pub enum ProviderDriver {
	OpenAi(Arc<OpenAiDriver>),
	Anthropic(Arc<AnthropicDriver>),
}

/// Builds the driver from CLI values + resolved credentials (WP-303 order:
/// stored `auth.json` credential, then ambient env var).
pub fn build_provider(name: &str, model: Option<&str>, agent_dir: &std::path::Path) -> Result<Option<ProviderDriver>> {
	match name {
		"fake" => Ok(None),
		"openai" => {
			let model = model.context("--provider openai requires --model <id>")?;
			let key = crate::auth::resolve_api_key("openai", agent_dir)?;
			let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
			Ok(Some(ProviderDriver::OpenAi(Arc::new(OpenAiDriver::new(base, key, model.to_string())))))
		}
		"anthropic" => {
			let model = model.context("--provider anthropic requires --model <id>")?;
			let key = crate::auth::resolve_api_key("anthropic", agent_dir)?;
			let base = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
			Ok(Some(ProviderDriver::Anthropic(Arc::new(AnthropicDriver::new(base, key, model.to_string())))))
		}
		other => anyhow::bail!("--provider must be fake|openai|anthropic (got {other})"),
	}
}

impl ModelDriver for Arc<OpenAiDriver> {
	fn complete<'a>(&'a self, convo: &'a [Value]) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
		self.as_ref().complete(convo)
	}
}

impl ModelDriver for Arc<AnthropicDriver> {
	fn complete<'a>(&'a self, convo: &'a [Value]) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
		self.as_ref().complete(convo)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn build_provider_requires_credentials() {
		let agent_dir = std::env::temp_dir().join(format!("pi-rust-bp-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&agent_dir).unwrap();
		let _ = std::env::remove_var("OPENAI_API_KEY");
		let _ = std::env::remove_var("ANTHROPIC_API_KEY");
		let err = build_provider("openai", Some("m"), &agent_dir).unwrap_err().to_string();
		assert!(err.contains("OPENAI_API_KEY"), "{err}");
		let err = build_provider("anthropic", Some("m"), &agent_dir).unwrap_err().to_string();
		assert!(err.contains("ANTHROPIC_API_KEY"), "{err}");
		let err = build_provider("openai", None, &agent_dir).unwrap_err().to_string();
		assert!(err.contains("--model"), "{err}");
		assert!(build_provider("bogus", Some("m"), &agent_dir).is_err());
		std::env::set_var("OPENAI_API_KEY", "test-key");
		match build_provider("openai", Some("m"), &agent_dir).unwrap().unwrap() {
			ProviderDriver::OpenAi(_) => {}
			_ => panic!("expected OpenAi"),
		}
		std::env::set_var("ANTHROPIC_API_KEY", "test-key");
		match build_provider("anthropic", Some("m"), &agent_dir).unwrap().unwrap() {
			ProviderDriver::Anthropic(_) => {}
			_ => panic!("expected Anthropic"),
		}
		std::env::remove_var("OPENAI_API_KEY");
		std::env::remove_var("ANTHROPIC_API_KEY");
	}
}

#[cfg(test)]
pub(crate) mod mock {
	/// Minimal mock: accepts ONE HTTP request, records it as
	/// `METHOD PATH\n<authorization line>\n<body>`, and answers with the raw HTTP payload.
	pub async fn start_mock(response: String, tx: tokio::sync::mpsc::Sender<String>) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		tokio::spawn(async move {
			let (mut sock, _) = match listener.accept().await {
				Ok(x) => x,
				Err(_) => return,
			};
			use tokio::io::{AsyncReadExt, AsyncWriteExt};
			let mut buf = Vec::new();
			let mut tmp = [0u8; 8192];
			loop {
				match sock.read(&mut tmp).await {
					Ok(0) | Err(_) => break,
					Ok(n) => {
						buf.extend_from_slice(&tmp[..n]);
						let s = String::from_utf8_lossy(&buf);
						if let Some(h) = s.find("\r\n\r\n") {
							let head = &s[..h];
							let len = head
								.lines()
								.find_map(|l| {
									let lower = l.to_lowercase();
									lower.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
								})
								.unwrap_or(0);
							if buf.len() >= h + 4 + len {
								break;
							}
						}
					}
				}
			}
			let s = String::from_utf8_lossy(&buf).to_string();
			let head = s.split("\r\n\r\n").next().unwrap_or("").to_string();
			let body = s.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default();
			let first = head.lines().next().unwrap_or_default();
			let mut parts = first.split_whitespace();
			let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
			let headers: Vec<String> = head
				.lines()
				.skip(1)
				.map(|l| l.trim().to_lowercase())
				.collect();
			let _ = tx.send(format!("{method} {path}\n{}\n---\n{body}", headers.join("\n"))).await;
			let _ = sock.write_all(response.as_bytes()).await;
			let _ = sock.shutdown().await;
		});
		format!("http://{addr}")
	}

	/// Wraps the SSE payload as a chunked 200 response (one chunk per line).
	pub fn sse_response(sse: &str) -> String {
		let mut out = String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n");
		for line in sse.split_inclusive('\n') {
			if line.trim().is_empty() {
				continue;
			}
			let b = line.as_bytes();
			out.push_str(&format!("{:x}\r\n", b.len()));
			out.push_str(line);
			out.push_str("\r\n");
		}
		out.push_str("0\r\n\r\n");
		out
	}

	/// A pi-format conversation exercising all message roles.
	pub fn test_convo() -> Vec<serde_json::Value> {
		vec![
			serde_json::json!({"role":"system","content":"SYS PROMPT","toolsAdded":[{"name":"bash","description":"d","parameters":{"type":"object"}}]}),
			serde_json::json!({"role":"user","content":[{"type":"text","text":"do it"}]}),
			serde_json::json!({"role":"assistant","content":[{"type":"text","text":"calling"},{"type":"toolCall","id":"tc_9","name":"bash","arguments":{"command":"ls"}}]}),
			serde_json::json!({"role":"toolResult","toolCallId":"tc_9","toolName":"bash","content":[{"type":"text","text":"file.txt"}],"isError":false}),
		]
	}
}
