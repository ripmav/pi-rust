//! API-key resolution (WP-303), verified upstream order
//! (`ai/src/auth/resolve.ts`: "A stored credential owns the provider"):
//! stored `auth.json` credential → ambient env var → typed error.
//!
//! v0 scope: `api_key` credentials with literal keys or `${ENV_VAR}`
//! references (credential `env` overlay shadows ambient env). Command
//! references are NOT executed (typed error); oauth credentials are a typed
//! "not supported in v0" error. Key material never appears in errors.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
/// Ambient env var per provider (subset of upstream `env-api-keys.ts`,
/// api-key auth only; unknown providers rely on auth.json or get a typed
/// error).
pub fn env_var_for(provider: &str) -> Option<&'static str> {
	Some(match provider {
		"openai" | "openai-codex" => "OPENAI_API_KEY",
		"anthropic" => "ANTHROPIC_API_KEY",
		"google" | "google-vertex" => "GEMINI_API_KEY",
		"azure" | "azure-openai" => "AZURE_OPENAI_API_KEY",
		"cloudflare-workers-ai" | "cloudflare-ai-gateway" => "CLOUDFLARE_API_KEY",
		"groq" => "GROQ_API_KEY",
		"xai" => "XAI_API_KEY",
		"openrouter" => "OPENROUTER_API_KEY",
		"huggingface" => "HF_TOKEN",
		"cerebras" => "CEREBRAS_API_KEY",
		"deepseek" => "DEEPSEEK_API_KEY",
		"mistral" => "MISTRAL_API_KEY",
		"minimax" => "MINIMAX_API_KEY",
		"together" => "TOGETHER_API_KEY",
		"fireworks" => "FIREWORKS_API_KEY",
		"zai" | "zai-coding-cn" => "ZAI_API_KEY",
		"bedrock" => "AWS_BEARER_TOKEN_BEDROCK",
		"openai-compat" => "OPENAI_API_KEY",
		_ => return None,
	})
}

/// Expands `${NAME}` references in a stored key: credential `env` overlay
/// first, then ambient env. Unresolvable references stay literal (they will
/// fail as a bad key at the provider, matching upstream leniency).
fn expand_refs(key: &str, cred_env: &HashMap<String, String>) -> String {
	let mut out = String::with_capacity(key.len());
	let mut rest = key;
	while let Some(i) = rest.find("${") {
		out.push_str(&rest[..i]);
		let after = &rest[i + 2..];
		let end = after.find('}').unwrap_or(usize::MAX);
		let name = &after[..end.min(after.len())];
		if after.len() > end && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() {
			if let Some(v) = cred_env.get(name).cloned().or_else(|| std::env::var(name).ok()) {
				out.push_str(&v);
			} else {
				out.push_str("${");
				out.push_str(name);
				out.push('}');
			}
			rest = &after[end + 1..];
		} else {
			out.push_str("${");
			rest = after;
		}
	}
	out.push_str(rest);
	out
}

/// Resolves the API key for `provider` from `<agent_dir>/auth.json` first,
/// then the ambient env var. Errors name the env var but never a key value.
pub fn resolve_api_key(provider: &str, agent_dir: &Path) -> Result<String> {
	let path = agent_dir.join("auth.json");
	if path.exists() {
		let raw = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
		let data: Value = serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
		if let Some(cred) = data.get(provider) {
			return match cred.get("type").and_then(Value::as_str) {
				Some("api_key") => {
					let key = cred
						.get("key")
						.and_then(Value::as_str)
						.context("auth.json credential for {provider} has no `key`")?;
					let cred_env: HashMap<String, String> = cred
						.get("env")
						.and_then(Value::as_object)
						.map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
						.unwrap_or_default();
					if key.contains("$(") {
						bail!("auth.json credential for {provider} uses a command reference — not supported in v0 (no shell execution from auth.json)");
					}
					Ok(expand_refs(key, &cred_env))
				}
				Some("oauth") => bail!("auth.json credential for {provider} is oauth — not supported in v0"),
				other => bail!("auth.json credential for {provider} has unknown type {other:?}"),
			};
		}
	}
	let env = env_var_for(provider)
		.ok_or_else(|| anyhow::anyhow!("no credentials for provider {provider}: nothing in auth.json and no known env var"))?;
	std::env::var(env)
		.with_context(|| format!("no credentials for provider {provider}: set {env} or store a credential in auth.json"))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn tmpdir() -> std::path::PathBuf {
		let d = std::env::temp_dir().join(format!("pi-rust-auth-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&d).unwrap();
		d
	}

	#[test]
	fn ambient_env_used_when_nothing_stored() {
		let d = tmpdir();
		std::env::set_var("PI_RUST_TEST_OPENAI_KEY", "env-key");
		// use the real var via a temporary rename: env_var_for is fixed to
		// OPENAI_API_KEY, so set/restore it
		let _old = std::env::var("OPENAI_API_KEY").ok();
		std::env::set_var("OPENAI_API_KEY", "env-key");
		let k = resolve_api_key("openai", &d).unwrap();
		assert_eq!(k, "env-key");
		match _old {
			Some(v) => std::env::set_var("OPENAI_API_KEY", v),
			None => std::env::remove_var("OPENAI_API_KEY"),
		}
		let _ = std::env::remove_var("PI_RUST_TEST_OPENAI_KEY");
	}

	#[test]
	fn stored_credential_beats_ambient_env() {
		let d = tmpdir();
		std::fs::write(d.join("auth.json"), r#"{"groq":{"type":"api_key","key":"stored-key"}}"#).unwrap();
		let _old = std::env::var("GROQ_API_KEY").ok();
		std::env::set_var("GROQ_API_KEY", "env-key");
		let k = resolve_api_key("groq", &d).unwrap();
		assert_eq!(k, "stored-key", "upstream: a stored credential owns the provider");
		match _old {
			Some(v) => std::env::set_var("GROQ_API_KEY", v),
			None => std::env::remove_var("GROQ_API_KEY"),
		}
	}

	#[test]
	fn stored_env_ref_expands_with_overlay() {
		let d = tmpdir();
		std::fs::write(d.join("auth.json"), r#"{"anthropic":{"type":"api_key","key":"${MY_KEY}","env":{"MY_KEY":"overlay-key"}}}"#).unwrap();
		let k = resolve_api_key("anthropic", &d).unwrap();
		assert_eq!(k, "overlay-key");
	}

	#[test]
	fn oauth_and_command_refs_are_typed_errors() {
		let d = tmpdir();
		std::fs::write(d.join("auth.json"), r#"{"openai":{"type":"oauth","access":"a"}}"#).unwrap();
		let err = resolve_api_key("openai", &d).unwrap_err().to_string();
		assert!(err.contains("oauth"), "{err}");
		std::fs::write(d.join("auth.json"), r#"{"openai":{"type":"api_key","key":"$(evil)"}}"#).unwrap();
		let err = resolve_api_key("openai", &d).unwrap_err().to_string();
		assert!(err.contains("command reference"), "{err}");
		assert!(!err.contains("evil"), "no material in error: {err}");
	}

	#[test]
	fn missing_everything_errors_with_env_name_not_key() {
		let d = tmpdir();
		let _old = std::env::var("XAI_API_KEY").ok();
		std::env::remove_var("XAI_API_KEY");
		let err = resolve_api_key("xai", &d).unwrap_err().to_string();
		assert!(err.contains("XAI_API_KEY"), "{err}");
		match _old {
			Some(v) => std::env::set_var("XAI_API_KEY", v),
			None => {}
		}
	}

	#[test]
	fn unknown_provider_without_credential_is_typed_error() {
		let d = tmpdir();
		let err = resolve_api_key("no-such-provider", &d).unwrap_err().to_string();
		assert!(err.contains("no-such-provider"), "{err}");
	}

	#[test]
	fn malformed_auth_json_errors_with_path() {
		let d = tmpdir();
		std::fs::write(d.join("auth.json"), "{nope").unwrap();
		let err = resolve_api_key("openai", &d).unwrap_err().to_string();
		assert!(err.contains("auth.json"), "{err}");
	}
}
