//! Real model provider drivers (OpenAI-compatible, Anthropic).
//! The `ModelDriver` seam keeps the agent loop provider-agnostic; the bridge
//! and sidecar never see provider details. Credentials come from the caller
//! (env vars at the CLI layer) — never from config files.
pub mod anthropic;
pub mod google;
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
	Google(Arc<google::GoogleDriver>),
}

impl ProviderDriver {
	/// Forwards `complete` to the wrapped driver (same pattern the agent
	/// loop uses; convenient for tests and future runtime switching).
	pub fn complete<'a>(
		&'a self,
		convo: &'a [Value],
	) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
		match self {
			ProviderDriver::OpenAi(d) => d.complete(convo),
			ProviderDriver::Anthropic(d) => d.complete(convo),
			ProviderDriver::Google(d) => d.complete(convo),
		}
	}
}

/// Builds the driver from CLI values + resolved credentials (WP-303 order:
/// stored `auth.json` credential, then ambient env var).
pub fn build_provider(name: &str, model: Option<&str>, agent_dir: &std::path::Path) -> Result<Option<ProviderDriver>> {
	match name {
		"fake" => Ok(None),
		"anthropic" => {
			let model = model.context("--provider anthropic requires --model <id>")?;
			let key = crate::auth::resolve_api_key("anthropic", agent_dir)?;
			let base = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
			Ok(Some(ProviderDriver::Anthropic(Arc::new(AnthropicDriver::new(base, key, model.to_string())))))
		}
		p if compat_family_base(p).is_some() => {
			let model = model.with_context(|| format!("--provider {p} requires --model <id>"))?;
			let key = crate::auth::resolve_api_key(p, agent_dir)?;
			let env_name = format!("{p}_BASE_URL").to_uppercase();
			let base = std::env::var(env_name.as_str())
				.unwrap_or_else(|_| compat_family_base(p).unwrap().to_string());
			Ok(Some(ProviderDriver::OpenAi(Arc::new(OpenAiDriver::new(base, key, model.to_string())))))
		}
		"azure" | "azure-openai" => {
			let name = "azure";
			let model = model.context("--provider azure requires --model <id>")?;
			let key = crate::auth::resolve_api_key(name, agent_dir)?;
			// base: AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME ->
			// https://{resource}.openai.azure.com/openai/v1 (upstream
			// azure-openai-config.ts: azure hosts get /openai/v1 as base path).
			let base = match (
				std::env::var("AZURE_OPENAI_BASE_URL").ok().filter(|b| !b.trim().is_empty()),
				std::env::var("AZURE_OPENAI_RESOURCE_NAME").ok().filter(|b| !b.trim().is_empty()),
			) {
				(Some(b), _) => b.trim().trim_end_matches('/').to_string(),
				(None, Some(resource)) => format!("https://{resource}.openai.azure.com/openai/v1"),
				(None, None) => {
					anyhow::bail!("azure requires AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME")
				}
			};
			// deployment: AZURE_OPENAI_DEPLOYMENT_NAME_MAP (model=deployment, comma
			// separated) or the catalog model id.
			let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP")
				.ok()
				.and_then(|map| {
					map.split(',')
						.filter_map(|e| {
							let (id, dep) = e.trim().split_once('=')?;
							if id.trim() == model {
								Some(dep.trim().to_string())
							} else {
								None
							}
						})
						.next()
				})
				.unwrap_or_else(|| model.to_string());
			let api_version = std::env::var("AZURE_OPENAI_API_VERSION")
				.ok()
				.filter(|v| !v.trim().is_empty())
				.unwrap_or_else(|| "v1".into());
			Ok(Some(ProviderDriver::OpenAi(Arc::new(
				OpenAiDriver::new_azure(base, key, model.to_string(), deployment, api_version),
			))))
		}
		"google" => {
			let model = model.context("--provider google requires --model <id>")?;
			let key = crate::auth::resolve_api_key("google", agent_dir)?;
			let base = std::env::var("GOOGLE_BASE_URL")
				.unwrap_or_else(|_| "https://generativelanguage.googleapis.com/v1beta".into());
			Ok(Some(ProviderDriver::Google(Arc::new(google::GoogleDriver::new(base, key, model.to_string())))))
		}
		"google-vertex" => {
			let model = model.context("--provider google-vertex requires --model <id>")?;
			let key = crate::auth::resolve_api_key("google-vertex", agent_dir)?;
			// v0: explicit API key only (x-goog-api-key). ADC/bearer needs the
			// gcloud token flow, which is a typed rejection here.
			let project = std::env::var("GOOGLE_CLOUD_PROJECT")
				.context("--provider google-vertex requires GOOGLE_CLOUD_PROJECT (API key auth; ADC is not supported in v0)")?;
			let location = std::env::var("GOOGLE_CLOUD_LOCATION")
				.context("--provider google-vertex requires GOOGLE_CLOUD_LOCATION (API key auth; ADC is not supported in v0)")?;
			let base = std::env::var("GOOGLE_VERTEX_BASE_URL")
				.unwrap_or_else(|_| format!("https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google"));
			Ok(Some(ProviderDriver::Google(Arc::new(google::GoogleDriver::new(base, key, model.to_string())))))
		}
		p if not_supported_v0(p).is_some() => {
			anyhow::bail!("provider {p} is not supported in v0 (upstream wire API: {})", not_supported_v0(p).unwrap())
		}
		other => anyhow::bail!("unknown provider {other} (supported: fake, anthropic, google, google-vertex, azure, openai, groq, cerebras, deepseek, together, huggingface, openrouter; cloudflare/bedrock arrive in later M4 WPs)"),
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
	fn build_google_and_vertex() {
		let agent_dir = std::env::temp_dir().join(format!("pi-rust-gbuild-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&agent_dir).unwrap();
		std::fs::write(
			agent_dir.join("auth.json"),
			serde_json::json!({"google":{"type":"api_key","key":"g-key"},
			                   "google-vertex":{"type":"api_key","key":"v-key"}}).to_string(),
		)
		.unwrap();
		// google: env base override + auth.json key.
		std::env::set_var("GOOGLE_BASE_URL", "http://127.0.0.1:1");
		match build_provider("google", Some("gemini-2.0-flash"), &agent_dir).unwrap().unwrap() {
			ProviderDriver::Google(_) => {}
			_ => panic!("expected Google"),
		}
		// vertex: project/location env required (typed error without them).
		std::env::remove_var("GOOGLE_CLOUD_PROJECT");
		std::env::remove_var("GOOGLE_CLOUD_LOCATION");
		let err = build_provider("google-vertex", Some("gemini-2.0-flash-001"), &agent_dir)
			.unwrap_err()
			.to_string();
		assert!(err.contains("GOOGLE_CLOUD_PROJECT"), "{err}");
		std::env::set_var("GOOGLE_CLOUD_PROJECT", "proj-x");
		std::env::set_var("GOOGLE_CLOUD_LOCATION", "us-central1");
		std::env::remove_var("GOOGLE_VERTEX_BASE_URL");
		match build_provider("google-vertex", Some("gemini-2.0-flash-001"), &agent_dir).unwrap().unwrap() {
			ProviderDriver::Google(_) => {}
			_ => panic!("expected Google (vertex)"),
		}
		std::env::remove_var("GOOGLE_BASE_URL");
		std::env::remove_var("GOOGLE_CLOUD_PROJECT");
		std::env::remove_var("GOOGLE_CLOUD_LOCATION");
	}


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

/// OpenAI-compatible (chat/completions) family table, verified against
/// upstream `packages/ai/src/providers/*.ts` (085c84e): provider id →
/// default base URL. These all use `openAICompletionsApi()` upstream.
pub fn compat_family_base(provider: &str) -> Option<&'static str> {
	Some(match provider {
		"openai" => "https://api.openai.com/v1",
		"groq" => "https://api.groq.com/openai/v1",
		"cerebras" => "https://api.cerebras.ai/v1",
		"deepseek" => "https://api.deepseek.com",
		"together" => "https://api.together.ai/v1",
		"huggingface" => "https://router.huggingface.co/v1",
		"openrouter" => "https://openrouter.ai/api/v1",
		_ => return None,
	})
}

/// Providers whose upstream wire API is NOT chat/completions: typed v0
/// rejection (name + wire API), not a silent wrong-family call.
pub fn not_supported_v0(provider: &str) -> Option<&'static str> {
	Some(match provider {
		"xai" => "responses",
		"minimax" | "minimax-cn" => "anthropic-messages",
		"mistral" => "conversations",
		"fireworks" => "custom",
		"openai-codex" => "codex-backend",
		"meta" => "custom",
		"baseten" => "custom",
		"github-copilot" => "copilot",
		_ => return None,
	})
}

#[cfg(test)]
mod family_tests {
	use super::*;

	#[test]
	fn compat_table_matches_upstream_defaults() {
		assert_eq!(compat_family_base("groq"), Some("https://api.groq.com/openai/v1"));
		assert_eq!(compat_family_base("cerebras"), Some("https://api.cerebras.ai/v1"));
		assert_eq!(compat_family_base("deepseek"), Some("https://api.deepseek.com"), "deepseek base has no /v1 (upstream)");
		assert_eq!(compat_family_base("together"), Some("https://api.together.ai/v1"));
		assert_eq!(compat_family_base("huggingface"), Some("https://router.huggingface.co/v1"));
		assert_eq!(compat_family_base("openrouter"), Some("https://openrouter.ai/api/v1"));
		assert_eq!(compat_family_base("openai"), Some("https://api.openai.com/v1"));
		assert_eq!(compat_family_base("xai"), None);
	}

	#[test]
	fn non_completions_families_are_typed_rejections() {
		assert_eq!(not_supported_v0("xai"), Some("responses"));
		assert_eq!(not_supported_v0("minimax"), Some("anthropic-messages"));
		assert_eq!(not_supported_v0("mistral"), Some("conversations"));
		assert_eq!(not_supported_v0("nope"), None);
	}

	/// One shared mock per family (sequential: env is process global).
	/// Per-family credential via temp agent-dir auth.json, base URL via the
	/// family's BASE_URL override env.
	#[tokio::test]
	async fn every_compat_family_reaches_mock_completions() {
		let agent_dir = std::env::temp_dir().join(format!("pi-rust-fam-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&agent_dir).unwrap();
		let families = ["groq", "cerebras", "deepseek", "together", "huggingface", "openrouter"];
		for p in families {
			let (tx, _rx) = tokio::sync::mpsc::channel(1);
			let sse = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"fam-ok\"}}]}\ndata: [DONE]\n";
			let base = mock::start_mock(mock::sse_response(sse), tx).await;
			let env_name = format!("{p}_BASE_URL").to_uppercase();
			std::fs::write(
				agent_dir.join("auth.json"),
				serde_json::json!({p: {"type": "api_key", "key": format!("key-{p}")}}).to_string(),
			)
			.unwrap();
			std::env::set_var(env_name.as_str(), &base);
			let drv = build_provider(p, Some("any-model"), &agent_dir)
				.unwrap_or_else(|e| panic!("{p}: {e}"))
				.expect("driver built");
			let turn = drv
				.complete(&[serde_json::json!({"role":"user","content":"hi"})])
				.await
				.unwrap_or_else(|e| panic!("{p} complete: {e}"));
			assert_eq!(turn.text.as_deref(), Some("fam-ok"), "{p}");
		}
	}
}
