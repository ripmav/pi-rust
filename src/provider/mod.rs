//! Real model provider drivers (OpenAI-compatible, Anthropic).
//! The `ModelDriver` seam keeps the agent loop provider-agnostic; the bridge
//! and sidecar never see provider details. Credentials come from the caller
//! (env vars at the CLI layer) — never from config files.
pub mod anthropic;
pub mod bedrock;
pub mod google;
pub mod openai;

pub use anthropic::AnthropicDriver;
pub use openai::OpenAiDriver;

use crate::model::{AssistantTurn, ModelDriver, StreamDelta};
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
    Bedrock(Arc<bedrock::BedrockDriver>),
}

impl ProviderDriver {
    /// Forwards `complete` to the wrapped driver; read by the in-crate tests.
    #[allow(dead_code)]
    pub fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        match self {
            ProviderDriver::OpenAi(d) => d.complete(convo, on_delta),
            ProviderDriver::Anthropic(d) => d.complete(convo, on_delta),
            ProviderDriver::Google(d) => d.complete(convo, on_delta),
            ProviderDriver::Bedrock(d) => d.complete(convo, on_delta),
        }
    }
}

/// Builds the driver from CLI values + resolved credentials (WP-303 order:
/// stored `auth.json` credential, then ambient env var).
/// WP-701: true for every provider name `build_provider` can serve (built-in
/// drivers incl. `fake`). Anything else may be an extension-registered
/// provider: the core defers to the sidecar `ready` payload and routes it via
/// the bridge (typed error if never registered).
pub fn is_builtin_provider(name: &str) -> bool {
    matches!(
        name,
        "fake"
            | "anthropic"
            | "azure"
            | "azure-openai"
            | "amazon-bedrock"
            | "cloudflare-workers-ai"
            | "cloudflare-ai-gateway"
            | "google"
            | "google-vertex"
    ) || compat_family_base(name).is_some()
        || not_supported_v0(name).is_some()
}

pub fn build_provider(
    name: &str,
    model: Option<&str>,
    agent_dir: &std::path::Path,
) -> Result<Option<ProviderDriver>> {
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
		"amazon-bedrock" => {
			let model = model.context("--provider amazon-bedrock requires --model <id>")?;
			// Env credentials only in v0 (profile files are a typed rejection).
			if std::env::var("AWS_PROFILE").is_ok() {
				anyhow::bail!("amazon-bedrock: AWS_PROFILE file credentials are not supported in v0 (env keys only)")
			}
			let access = std::env::var("AWS_ACCESS_KEY_ID")
				.context("amazon-bedrock requires AWS_ACCESS_KEY_ID (env credentials; AWS_PROFILE files are not supported in v0)")?;
			let secret = std::env::var("AWS_SECRET_ACCESS_KEY")
				.context("amazon-bedrock requires AWS_SECRET_ACCESS_KEY")?;
			let token = std::env::var("AWS_SESSION_TOKEN").ok().filter(|t| !t.is_empty());
			let region = std::env::var("AWS_REGION")
				.ok()
				.filter(|r| !r.is_empty())
				.unwrap_or_else(|| "us-east-1".into());
			Ok(Some(ProviderDriver::Bedrock(Arc::new(
				bedrock::BedrockDriver::new(region, access, secret, token, model.to_string()),
			))))
		}
		"cloudflare-workers-ai" | "cloudflare-ai-gateway" => {
			let model = model
				.with_context(|| format!("--provider {name} requires --model <id>"))?;
			let key = crate::auth::resolve_api_key(name, agent_dir)?;
			let account = std::env::var("CLOUDFLARE_ACCOUNT_ID")
				.with_context(|| format!("{name} requires CLOUDFLARE_ACCOUNT_ID"))?;
			// Upstream catalog baseUrl placeholders, materialized from env:
			// workers-ai -> .../ai/v1 (OpenAI-compatible), gateway chat ->
			// .../compat (unified API), both OpenAI-completions wire.
			let base = match name {
				"cloudflare-workers-ai" => {
					format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/v1")
				}
				_ => {
					let gateway = std::env::var("CLOUDFLARE_GATEWAY_ID")
						.context("cloudflare-ai-gateway requires CLOUDFLARE_GATEWAY_ID")?;
					format!("https://gateway.ai.cloudflare.com/v1/{account}/{gateway}/compat")
				}
			};
			Ok(Some(ProviderDriver::OpenAi(Arc::new(OpenAiDriver::new(base, key, model.to_string())))))
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
		other => anyhow::bail!("unknown provider {other} (supported: fake, anthropic, google, google-vertex, azure, cloudflare-workers-ai, cloudflare-ai-gateway, amazon-bedrock, openai, groq, cerebras, deepseek, together, huggingface, openrouter)"),
	}
}

impl ModelDriver for Arc<OpenAiDriver> {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        self.as_ref().complete(convo, on_delta)
    }
}

impl ModelDriver for Arc<AnthropicDriver> {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        self.as_ref().complete(convo, on_delta)
    }
}

// ---------------------------------------------------------------------------
// WP-904: provider error classes + one automatic retry.

/// Classified provider failure. The user message never contains the API key
/// or a URL (redaction requirement).
#[derive(Clone, Debug)]
pub enum ProviderFailure {
    /// Non-retryable HTTP error (401/403 and the other 4xx).
    Http { status: u16, body: String },
    /// HTTP 429 — retryable once, honoring Retry-After.
    RateLimited { retry_after_secs: u64, body: String },
    /// HTTP 5xx — retryable once.
    Server { status: u16, body: String },
    /// Transport-level failure (connect/timeout/reset) — retryable once.
    Network { detail: String },
}

impl ProviderFailure {
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Server { .. } | Self::Network { .. }
        )
    }

    /// Delay before the single automatic retry (Retry-After capped, else 1s).
    pub fn backoff(&self) -> std::time::Duration {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => std::time::Duration::from_secs((*retry_after_secs).min(30)),
            _ => std::time::Duration::from_secs(1),
        }
    }

    /// Actionable, redacted user-facing message.
    pub fn message(&self, name: &str) -> String {
        let clip = |b: &str| b.chars().take(160).collect::<String>();
        match self {
            Self::Http { status, body } => {
                if *status == 401 || *status == 403 {
                    let mut m = format!(
                        "provider {name} rejected the API key (HTTP {status}) — check the stored credential or the environment variable"
                    );
                    if !body.is_empty() {
                        m.push_str(&format!(" (provider: {})", clip(body)));
                    }
                    m
                } else {
                    let mut m = format!("provider {name} error (HTTP {status})");
                    if !body.is_empty() {
                        m.push_str(&format!(": {}", clip(body)));
                    }
                    m
                }
            }
            Self::RateLimited { retry_after_secs, body } => {
                let mut m = format!(
                    "provider {name} rate limit (HTTP 429) — one automatic retry also hit the limit; try again in ~{retry_after_secs}s"
                );
                if !body.is_empty() {
                    m.push_str(&format!(" (provider: {})", clip(body)));
                }
                m
            }
            Self::Server { status, body } => {
                let mut m = format!(
                    "provider {name} returned HTTP {status} — one automatic retry also failed"
                );
                if !body.is_empty() {
                    m.push_str(&format!(" (provider: {})", clip(body)));
                }
                m
            }
            Self::Network { detail } => format!(
                "network error talking to provider {name} ({detail}) — one automatic retry also failed"
            ),
        }
    }
}

/// Send a provider request with ONE automatic retry for transient failures
/// (429 honoring Retry-After, 5xx, network errors). `build` recreates the
/// request per attempt (`send` consumes the builder).
pub async fn send_with_retry(
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response, ProviderFailure> {
    for attempt in 0..2u8 {
        let failure = match build().send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp);
                }
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(5);
                let body = resp.text().await.unwrap_or_default();
                match status.as_u16() {
                    429 => ProviderFailure::RateLimited {
                        retry_after_secs: retry_after,
                        body,
                    },
                    s if (500..600).contains(&s) => ProviderFailure::Server { status: s, body },
                    s => ProviderFailure::Http { status: s, body },
                }
            }
            Err(e) => {
                let detail = if e.is_timeout() {
                    "timeout"
                } else if e.is_connect() {
                    "connection failed"
                } else {
                    "request failed"
                };
                ProviderFailure::Network {
                    detail: detail.into(),
                }
            }
        };
        if !failure.retryable() || attempt == 1 {
            return Err(failure);
        }
        tokio::time::sleep(failure.backoff()).await;
    }
    unreachable!("the loop returns on attempt 0 or 1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// HTTP mock: serves queued (status, retry-after, body) per request and
    /// counts the requests. One request per connection (the test clients do
    /// not pool).
    fn status_mock(
        responses: Vec<(u16, Option<u64>, &str)>,
        max: usize,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        let responses: Vec<(u16, Option<u64>, String)> = responses
            .into_iter()
            .map(|(st, ra, b)| (st, ra, b.to_string()))
            .collect();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for n in 0..max {
                let Ok((mut s, _)) = listener.accept() else {
                    break;
                };
                let mut buf = vec![0u8; 65536];
                let mut have = 0usize;
                while !buf[..have].windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf[have..]) {
                        Ok(k) if k > 0 => have += k,
                        _ => break,
                    }
                    if have == buf.len() {
                        buf.resize(have * 2, 0);
                    }
                }
                if have == 0 {
                    break;
                }
                let (status, retry_after, body) = responses[n % responses.len()].clone();
                hits2.fetch_add(1, Ordering::SeqCst);
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n",
                    body.len()
                );
                if let Some(secs) = retry_after {
                    resp += &format!("retry-after: {secs}\r\n");
                }
                resp += "\r\n";
                resp += &body;
                let _ = s.write_all(resp.as_bytes());
                let _ = s.flush();
            }
        });
        (base, hits)
    }

    const ECHO_SSE: &str = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"echo\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

    fn test_convo() -> Vec<Value> {
        vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})]
    }

    /// WP-904: 401 → key guidance, no retry, redacted.
    #[tokio::test]
    async fn wp904_401_fails_fast_with_key_guidance() {
        let (base, hits) = status_mock(vec![(401, None, "Invalid API key provided")], 4);
        let d = OpenAiDriver::new(base, "SECRET-KEY-123".into(), "gpt-4o-mini".into());
        let err = d
            .complete(&test_convo(), Box::new(|_| {}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("rejected the API key"), "{err}");
        assert!(!err.contains("SECRET-KEY-123"), "redaction: {err}");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "401 must not be retried");
    }

    /// WP-904: 429 honors Retry-After, the retry succeeds.
    #[tokio::test]
    async fn wp904_429_honors_retry_after_then_succeeds() {
        let (base, hits) = status_mock(vec![(429, Some(1), "slow down"), (200, None, ECHO_SSE)], 4);
        let d = OpenAiDriver::new(base, "k".into(), "gpt-4o-mini".into());
        let t = std::time::Instant::now();
        let turn = d
            .complete(&test_convo(), Box::new(|_| {}))
            .await
            .expect("the retried request succeeds");
        assert!(
            t.elapsed() >= std::time::Duration::from_millis(900),
            "retry-after not honored: {:?}",
            t.elapsed()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(turn.text.as_deref(), Some("echo"));
    }

    /// WP-904: 429 twice → rate-limit guidance after the one retry.
    #[tokio::test]
    async fn wp904_429_twice_fails_with_rate_limit_guidance() {
        let (base, hits) = status_mock(vec![(429, Some(1), "slow down")], 4);
        let d = OpenAiDriver::new(base, "k".into(), "gpt-4o-mini".into());
        let err = d
            .complete(&test_convo(), Box::new(|_| {}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("rate limit") && err.contains("429"), "{err}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    /// WP-904: 5xx twice → one retry, then a clean failure.
    #[tokio::test]
    async fn wp904_5xx_retries_once_then_fails() {
        let (base, hits) = status_mock(vec![(500, None, "boom")], 4);
        let d = OpenAiDriver::new(base, "k".into(), "gpt-4o-mini".into());
        let err = d
            .complete(&test_convo(), Box::new(|_| {}))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 500") && err.contains("retry"), "{err}");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "exactly one retry");
    }

    /// WP-904: redaction — no key and no creds-bearing URL in error text.
    #[tokio::test]
    async fn wp904_redaction_never_leaks_key_or_url_creds() {
        let (base, _hits) = status_mock(vec![(401, None, "nope")], 4);
        let port = base.rsplit_once(':').unwrap().1;
        let creds_base = format!("http://user:pass@127.0.0.1:{port}");
        let d = OpenAiDriver::new(creds_base, "SECRET-KEY-123".into(), "gpt-4o-mini".into());
        let err = d
            .complete(&test_convo(), Box::new(|_| {}))
            .await
            .unwrap_err()
            .to_string();
        assert!(!err.contains("SECRET-KEY-123"), "key leaked: {err}");
        assert!(!err.contains("user:pass@"), "url creds leaked: {err}");
        assert!(err.contains("rejected the API key"), "{err}");
    }

    #[test]
    fn build_cloudflare_requires_account_and_gateway() {
        let agent_dir = std::env::temp_dir().join(format!("pi-rust-cf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("auth.json"),
            serde_json::json!({"cloudflare-workers-ai":{"type":"api_key","key":"cf-key"},
			                   "cloudflare-ai-gateway":{"type":"api_key","key":"cf-key"}})
            .to_string(),
        )
        .unwrap();
        // workers-ai: account required.
        std::env::remove_var("CLOUDFLARE_ACCOUNT_ID");
        let err = build_provider("cloudflare-workers-ai", Some("@cf/llama"), &agent_dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("CLOUDFLARE_ACCOUNT_ID"), "{err}");
        // gateway: account AND gateway id required.
        std::env::set_var("CLOUDFLARE_ACCOUNT_ID", "acct1");
        std::env::remove_var("CLOUDFLARE_GATEWAY_ID");
        let err = build_provider("cloudflare-ai-gateway", Some("openai/gpt-4o"), &agent_dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("CLOUDFLARE_GATEWAY_ID"), "{err}");
        std::env::set_var("CLOUDFLARE_GATEWAY_ID", "gw1");
        // both build (driver internals verified by mock tests).
        match build_provider("cloudflare-workers-ai", Some("@cf/llama"), &agent_dir)
            .unwrap()
            .unwrap()
        {
            ProviderDriver::OpenAi(_) => {}
            _ => panic!("expected OpenAi (workers-ai)"),
        }
        match build_provider("cloudflare-ai-gateway", Some("openai/gpt-4o"), &agent_dir)
            .unwrap()
            .unwrap()
        {
            ProviderDriver::OpenAi(_) => {}
            _ => panic!("expected OpenAi (gateway)"),
        }
        std::env::remove_var("CLOUDFLARE_ACCOUNT_ID");
        std::env::remove_var("CLOUDFLARE_GATEWAY_ID");
    }

    #[test]
    fn build_google_and_vertex() {
        let agent_dir =
            std::env::temp_dir().join(format!("pi-rust-gbuild-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(
            agent_dir.join("auth.json"),
            serde_json::json!({"google":{"type":"api_key","key":"g-key"},
			                   "google-vertex":{"type":"api_key","key":"v-key"}})
            .to_string(),
        )
        .unwrap();
        // google: env base override + auth.json key.
        std::env::set_var("GOOGLE_BASE_URL", "http://127.0.0.1:1");
        match build_provider("google", Some("gemini-2.0-flash"), &agent_dir)
            .unwrap()
            .unwrap()
        {
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
        match build_provider("google-vertex", Some("gemini-2.0-flash-001"), &agent_dir)
            .unwrap()
            .unwrap()
        {
            ProviderDriver::Google(_) => {}
            _ => panic!("expected Google (vertex)"),
        }
        std::env::remove_var("GOOGLE_BASE_URL");
        std::env::remove_var("GOOGLE_CLOUD_PROJECT");
        std::env::remove_var("GOOGLE_CLOUD_LOCATION");
    }

    #[test]
    fn build_provider_requires_credentials() {
        let agent_dir = std::env::temp_dir().join(format!(
            "pi-rust-bp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
        let err = build_provider("openai", Some("m"), &agent_dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("OPENAI_API_KEY"), "{err}");
        let err = build_provider("anthropic", Some("m"), &agent_dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ANTHROPIC_API_KEY"), "{err}");
        let err = build_provider("openai", None, &agent_dir)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--model"), "{err}");
        assert!(build_provider("bogus", Some("m"), &agent_dir).is_err());
        std::env::set_var("OPENAI_API_KEY", "test-key");
        match build_provider("openai", Some("m"), &agent_dir)
            .unwrap()
            .unwrap()
        {
            ProviderDriver::OpenAi(_) => {}
            _ => panic!("expected OpenAi"),
        }
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        match build_provider("anthropic", Some("m"), &agent_dir)
            .unwrap()
            .unwrap()
        {
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
        start_mock_bytes(response.into_bytes(), tx).await
    }

    /// Byte-exact variant for non-UTF8 responses (e.g. AWS event streams).
    pub async fn start_mock_bytes(
        response: Vec<u8>,
        tx: tokio::sync::mpsc::Sender<String>,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut recorded = false;
            loop {
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
                                        lower
                                            .strip_prefix("content-length:")
                                            .and_then(|v| v.trim().parse::<usize>().ok())
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
                let body = s
                    .split_once("\r\n\r\n")
                    .map(|x| x.1.to_string())
                    .unwrap_or_default();
                let first = head.lines().next().unwrap_or_default();
                let mut parts = first.split_whitespace();
                let (method, path) = (
                    parts.next().unwrap_or("").to_string(),
                    parts.next().unwrap_or("").to_string(),
                );
                let headers: Vec<String> = head
                    .lines()
                    .skip(1)
                    .map(|l| l.trim().to_lowercase())
                    .collect();
                // Record only the first request: test channels are sized 1 and
                // a full channel would stall the mock on a retried request.
                if !recorded {
                    let _ = tx
                        .send(format!(
                            "{method} {path}\n{}\n---\n{body}",
                            headers.join("\n")
                        ))
                        .await;
                    recorded = true;
                }
                let _ = sock.write_all(&response).await;
                let _ = sock.shutdown().await;
            }
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
/// upstream `packages/ai/src/providers/*.ts` (17d6ca7): provider id →
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
        assert_eq!(
            compat_family_base("groq"),
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(
            compat_family_base("cerebras"),
            Some("https://api.cerebras.ai/v1")
        );
        assert_eq!(
            compat_family_base("deepseek"),
            Some("https://api.deepseek.com"),
            "deepseek base has no /v1 (upstream)"
        );
        assert_eq!(
            compat_family_base("together"),
            Some("https://api.together.ai/v1")
        );
        assert_eq!(
            compat_family_base("huggingface"),
            Some("https://router.huggingface.co/v1")
        );
        assert_eq!(
            compat_family_base("openrouter"),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            compat_family_base("openai"),
            Some("https://api.openai.com/v1")
        );
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
        let families = [
            "groq",
            "cerebras",
            "deepseek",
            "together",
            "huggingface",
            "openrouter",
        ];
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
                .complete(
                    &[serde_json::json!({"role":"user","content":"hi"})],
                    Box::new(|_: crate::model::StreamDelta| {}),
                )
                .await
                .unwrap_or_else(|e| panic!("{p} complete: {e}"));
            assert_eq!(turn.text.as_deref(), Some("fam-ok"), "{p}");
        }
    }
}
