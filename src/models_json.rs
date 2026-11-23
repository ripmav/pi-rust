//! Core-native custom providers from `<agent-dir>/models.json` (WP-1401, E3S1).
//!
//! Upstream parity target: the pinned upstream
//! `packages/coding-agent/src/core/model-config.ts` (64a15f4) — the schema
//! subset the core consumes natively:
//! `providers{name, baseUrl, api, apiKey, models[{id, name, reasoning, input,
//! cost, contextWindow, maxTokens}]}`. JSONC like upstream: BOM + `//` and
//! `/* */` comments + trailing commas, string literals untouched. Unknown
//! fields are ignored (forward compat).
//!
//! Missing file = no-op; malformed file = typed `ModelsJsonError` — the run
//! continues with the bundled catalog (the caller logs the error, E3S1).
//!
//! Core ownership (E3S1): a `(provider, model)` is core-native when
//! [`ModelsJson::owner`] returns the provider definition — the model is
//! listed in the provider's `models`, the provider `api` is
//! `openai-completions` (the core's built-in OpenAI driver), and the
//! documented v0 limit holds (E2S1): for a core-builtin provider name, the
//! bundled catalog entry wins per model id. Other api types (e.g.
//! `anthropic-messages`) are a typed "not core-native yet" rejection —
//! never a crash.
//!
//! Credential: the provider's `apiKey` (literal or `$ENV_VAR` /
//! `${ENV_VAR}` interpolation, upstream `resolveConfigValue` template
//! semantics minus shell `!command`, which is a typed rejection) resolves
//! first; without it, the existing core credential store applies
//! (auth.json → env, [`crate::auth::resolve_api_key`]).

use crate::provider::{OpenAiDriver, ProviderDriver};
use crate::registry::{Cost, Model, ModelRegistry};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// One `<agent-dir>/models.json` provider (upstream `ModelsJsonProvider`
/// subset; unknown fields ignored).
#[derive(Debug, Clone, PartialEq)]
pub struct Provider {
    /// Provider id (the `providers` map key; the registry provider name).
    pub id: String,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub api_key: Option<String>,
    /// Core-registry-shaped models: `api`/`provider` pre-filled, upstream
    /// defaults applied (reasoning `false`, input `["text"]`, cost 0,
    /// contextWindow 128000, maxTokens 16384).
    pub models: Vec<Model>,
}

/// The loaded file (missing file = empty).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelsJson {
    providers: BTreeMap<String, Provider>,
}

/// Typed models.json load error (E3S1: the run continues with the bundled
/// catalog; the message names the path).
#[derive(Debug)]
pub enum ModelsJsonError {
    /// The file could not be read (IO error; ENOENT is a no-op, not an error).
    Read(std::io::Error),
    /// Valid JSONC strip, invalid JSON (message includes the serde detail).
    Parse(String),
    /// Valid JSON, wrong shape (message includes the offending path).
    Schema(String),
}

impl fmt::Display for ModelsJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => write!(f, "models.json: {e}"),
            Self::Parse(e) => write!(f, "models.json: {e}"),
            Self::Schema(e) => write!(f, "models.json: {e}"),
        }
    }
}

impl std::error::Error for ModelsJsonError {}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ProviderRaw {
    base_url: Option<String>,
    api: Option<String>,
    api_key: Option<String>,
    models: Option<Vec<ModelRaw>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ModelRaw {
    id: String,
    name: Option<String>,
    reasoning: Option<bool>,
    input: Option<Vec<String>>,
    cost: Option<Cost>,
    context_window: Option<u64>,
    max_tokens: Option<u64>,
}

/// Loads `<agent_dir>/models.json`. Missing file → `Ok` empty (no-op).
/// Malformed file → typed `Err` (the caller logs and continues).
pub fn load(agent_dir: &Path) -> std::result::Result<ModelsJson, ModelsJsonError> {
    let path = agent_dir.join("models.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ModelsJson::default()),
        Err(e) => return Err(ModelsJsonError::Read(e)),
    };
    let stripped = strip_jsonc(&raw);
    let parsed: serde_json::Value = serde_json::from_str(&stripped)
        .map_err(|e| ModelsJsonError::Parse(format!("{}: invalid JSON ({e})", path.display())))?;
    let Some(root) = parsed.as_object() else {
        return Err(ModelsJsonError::Schema(format!(
            "{}: top level must be an object",
            path.display()
        )));
    };
    let Some(providers) = root.get("providers") else {
        return Ok(ModelsJson::default()); // valid file, no providers
    };
    let Some(entries) = providers.as_object() else {
        return Err(ModelsJsonError::Schema(format!(
            "{}: `providers` must be an object",
            path.display()
        )));
    };
    let mut out = BTreeMap::new();
    for (id, entry) in entries {
        if !entry.is_object() {
            return Err(ModelsJsonError::Schema(format!(
                "{path}: provider `{id}` must be an object",
                path = path.display()
            )));
        }
        let raw: ProviderRaw = serde_json::from_value(entry.clone()).map_err(|e| {
            ModelsJsonError::Schema(format!(
                "{path}: provider `{id}`: {e}",
                path = path.display()
            ))
        })?;
        let api = raw.api.clone();
        let models = raw
            .models
            .unwrap_or_default()
            .into_iter()
            .map(|m| Model {
                id: m.id.clone(),
                name: m.name.unwrap_or_else(|| m.id.clone()),
                api: api.clone().unwrap_or_default(),
                provider: id.clone(),
                base_url: raw.base_url.clone(),
                reasoning: m.reasoning.unwrap_or(false),
                input: m.input.unwrap_or_else(|| vec!["text".to_string()]),
                cost: m.cost.unwrap_or_default(),
                // Upstream `model-config` defaults: a model without the
                // fields still gets them (compaction settings rely on a
                // Some contextWindow).
                context_window: m.context_window.or(Some(128_000)),
                max_tokens: m.max_tokens.or(Some(16_384)),
                thinking_level_map: None,
            })
            .collect();
        out.insert(
            id.clone(),
            Provider {
                id: id.clone(),
                base_url: raw.base_url,
                api,
                api_key: raw.api_key,
                models,
            },
        );
    }
    Ok(ModelsJson { providers: out })
}

/// BOM + `//` line comments + `/* */` block comments + trailing commas,
/// string literals untouched (upstream `stripJsonComments` semantics plus
/// block comments, which the E3S1 acceptance names). Two passes: comments
/// first (string-aware), then trailing commas on the comment-free text
/// (upstream order — a comment may sit between the comma and the brace).
pub fn strip_jsonc(input: &str) -> String {
    strip_trailing_commas(&strip_comments(
        input.strip_prefix('\u{feff}').unwrap_or(input),
    ))
}

/// Drops `//` and `/* */` comments, string literals untouched.
fn strip_comments(input: &str) -> String {
    let c: Vec<char> = input.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        match c[i] {
            '"' => copy_string(&c, &mut i, &mut out),
            '/' if i + 1 < n && c[i + 1] == '/' => {
                while i < n && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < n && c[i + 1] == '*' => {
                i += 2;
                while i + 1 < n && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Copies the string literal whose opening quote is at `c[*i]` verbatim
/// into `out` and advances `*i` past the closing quote (string interiors
/// are never inspected — shared by both JSONC passes).
fn copy_string(c: &[char], i: &mut usize, out: &mut String) {
    let n = c.len();
    out.push('"');
    *i += 1;
    while *i < n {
        if c[*i] == '\\' && *i + 1 < n {
            out.push(c[*i]);
            out.push(c[*i + 1]);
            *i += 2;
            continue;
        }
        out.push(c[*i]);
        *i += 1;
        if c[*i - 1] == '"' {
            break;
        }
    }
}

/// Trailing commas (upstream tolerance): a `,` whose only following
/// non-whitespace char is `}` or `]`, string literals untouched.
fn strip_trailing_commas(input: &str) -> String {
    let c: Vec<char> = input.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        match c[i] {
            '"' => copy_string(&c, &mut i, &mut out),
            ',' => {
                let mut k = i + 1;
                while k < n && c[k].is_whitespace() {
                    k += 1;
                }
                i += 1;
                if !(k < n && (c[k] == '}' || c[k] == ']')) {
                    out.push(',');
                }
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

impl ModelsJson {
    /// Read by the in-crate tests.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Read by the in-crate tests.
    #[allow(dead_code)]
    pub fn providers(&self) -> &BTreeMap<String, Provider> {
        &self.providers
    }

    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.get(name)
    }

    /// The registry entry of a listed models.json model (any api).
    pub fn model(&self, provider: &str, id: &str) -> Option<&Model> {
        self.provider(provider)?.models.iter().find(|m| m.id == id)
    }

    /// The core-native owner of `(provider, model)`: the models.json
    /// provider definition when the core must serve the model itself —
    /// `Some` when the model is listed, the provider `api` is
    /// `openai-completions`, and (documented v0 limit, E2S1) the model id is
    /// not a bundled-catalog entry of a core-builtin provider.
    pub fn owner(&self, provider: &str, model: &str) -> Option<&Provider> {
        let prov = self.provider(provider)?;
        if !prov.models.iter().any(|m| m.id == model) {
            return None;
        }
        if prov.api.as_deref() != Some("openai-completions") {
            return None; // not core-native yet (other api types)
        }
        if crate::provider::is_builtin_provider(provider)
            && ModelRegistry::bundled().resolve(provider, model).is_some()
        {
            return None; // bundled catalog wins for that id (E2S1 limit)
        }
        Some(prov)
    }

    /// The registry entries of every core-native model (picker/listing).
    pub fn core_models(&self) -> Vec<Model> {
        self.providers
            .values()
            .flat_map(|p| p.models.iter())
            .filter(|m| self.owner(&m.provider, &m.id).is_some())
            .cloned()
            .collect()
    }
}

/// Builds the core driver for a core-owned models.json model (E3S1): the
/// built-in OpenAI driver on the provider's `baseUrl` + resolved `apiKey`.
/// Typed errors (no crash): non-`openai-completions` api, missing `baseUrl`,
/// `!command` keys, unresolvable `$ENV_VAR` references, no credential.
pub fn build_driver(prov: &Provider, model_id: &str, agent_dir: &Path) -> Result<ProviderDriver> {
    if prov.api.as_deref() != Some("openai-completions") {
        bail!(
            "models.json provider `{}` uses api {:?}, which is not core-native yet (the core streams `openai-completions` only)",
            prov.id,
            prov.api.clone().unwrap_or_default()
        );
    }
    let base = prov
        .base_url
        .clone()
        .filter(|b| !b.trim().is_empty())
        .with_context(|| format!("models.json provider `{}`: `baseUrl` is required", prov.id))?;
    let key = resolve_key(prov, agent_dir)?;
    Ok(ProviderDriver::OpenAi(Arc::new(OpenAiDriver::new(
        base,
        key,
        model_id.to_string(),
    ))))
}

/// Provider `apiKey` (literal or env template) first; without it, the
/// existing core credential store (auth.json → env).
fn resolve_key(prov: &Provider, agent_dir: &Path) -> Result<String> {
    match &prov.api_key {
        Some(t) if t.starts_with('!') => bail!(
            "models.json provider `{}` uses a `!command` apiKey — shell commands are not supported (use a literal key or $ENV_VAR)",
            prov.id
        ),
        Some(t) => resolve_template(t)
            .with_context(|| format!("models.json provider `{}` apiKey", prov.id)),
        None => crate::auth::resolve_api_key(&prov.id, agent_dir),
    }
}

/// Upstream `resolveConfigValue` template subset (`$NAME`, `${NAME}`,
/// `$$`/`$!` escapes; mixed literals). A referenced env var that is unset is
/// a typed error naming it (upstream `resolveConfigValueOrThrow`).
pub fn resolve_template(value: &str) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    let c: Vec<char> = value.chars().collect();
    let n = c.len();
    let mut i = 0;
    while i < n {
        if c[i] != '$' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let next = c.get(i + 1);
        match next {
            Some('$') => {
                out.push('$');
                i += 2;
            }
            Some('!') => {
                out.push('$');
                out.push('!');
                i += 2;
            }
            Some('{') => {
                let end = c[i + 2..]
                    .iter()
                    .position(|&ch| ch == '}')
                    .map(|p| i + 2 + p);
                match end {
                    Some(end)
                        if c[i + 2..end]
                            .iter()
                            .all(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                            && !c[i + 2..end].is_empty() =>
                    {
                        let name: String = c[i + 2..end].iter().collect();
                        match std::env::var(&name) {
                            Ok(v) => out.push_str(&v),
                            Err(_) => bail!("apiKey references env var {name} which is not set"),
                        }
                        i = end + 1;
                    }
                    _ => {
                        // Not a valid ${NAME}: keep verbatim (upstream).
                        out.push('$');
                        i += 1;
                    }
                }
            }
            Some(first) if first.is_ascii_alphabetic() || *first == '_' => {
                let mut end = i + 1;
                while end < n && (c[end].is_ascii_alphanumeric() || c[end] == '_') {
                    end += 1;
                }
                let name: String = c[i + 1..end].iter().collect();
                match std::env::var(&name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => bail!("apiKey references env var {name} which is not set"),
                }
                i = end;
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-mj-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const SOLHEIM: &str = r#"{
  // solheim case (JSONC, BOM-free here)
  "providers": {
    "solheim": {
      "name": "Solheim",
      "baseUrl": "https://api.solheim.ai/v1",
      "api": "openai-completions",
      "apiKey": "$SOLHEIM_API_KEY",
      "models": [
        { "id": "qwen3.8-27b", "name": "Qwen3.8 27B", "reasoning": false,
          "input": ["text"], "cost": {"input": 0, "output": 0},
          "contextWindow": 262144, "maxTokens": 65534 }
      ]
    }
  }
}
"#;

    #[test]
    fn strip_jsonc_handles_bom_comments_and_trailing_commas() {
        let raw = "\u{feff}{\n/* block\ncomment */\n\"k\": \"v // not a comment, ] /* keep\", /* tail */\n\"k2\": [1, 2,], // line\n}\n";
        let out = strip_jsonc(raw);
        assert_eq!(
            out, "{\n\n\"k\": \"v // not a comment, ] /* keep\", \n\"k2\": [1, 2] \n}\n",
            "string contents stay, comments + trailing commas go"
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["k"], "v // not a comment, ] /* keep");
        assert_eq!(v["k2"], serde_json::json!([1, 2]));
    }

    #[test]
    fn missing_file_is_noop() {
        let d = tmpdir();
        let mj = load(&d).unwrap();
        assert!(mj.is_empty());
    }

    #[test]
    fn malformed_file_is_typed_error() {
        let d = tmpdir();
        std::fs::write(d.join("models.json"), "{nope").unwrap();
        assert!(matches!(load(&d), Err(ModelsJsonError::Parse(_))));
        std::fs::write(d.join("models.json"), r#"{"providers": []}"#).unwrap();
        assert!(matches!(load(&d), Err(ModelsJsonError::Schema(_))));
        std::fs::write(d.join("models.json"), r#"{"providers": {"p": "x"}}"#).unwrap();
        let e = load(&d).unwrap_err().to_string();
        assert!(e.contains("`p`"), "{e}");
    }

    #[test]
    fn parses_solheim_shape_with_defaults() {
        let d = tmpdir();
        std::fs::write(d.join("models.json"), SOLHEIM).unwrap();
        let mj = load(&d).unwrap();
        let p = mj.provider("solheim").expect("solheim provider");
        assert_eq!(p.base_url.as_deref(), Some("https://api.solheim.ai/v1"));
        assert_eq!(p.api.as_deref(), Some("openai-completions"));
        assert_eq!(p.api_key.as_deref(), Some("$SOLHEIM_API_KEY"));
        let m = &p.models[0];
        assert_eq!(m.id, "qwen3.8-27b");
        assert_eq!(m.name, "Qwen3.8 27B");
        assert_eq!(m.provider, "solheim");
        assert_eq!(m.api, "openai-completions");
        assert_eq!(m.context_window, Some(262144));
        assert_eq!(m.max_tokens, Some(65534));
        assert_eq!(m.input, vec!["text"]);
        assert!(!m.reasoning);
        // Defaults applied (upstream): absent cost fields are 0.
        assert_eq!(m.cost.cache_read, 0.0);
    }

    #[test]
    fn missing_window_and_tokens_get_upstream_defaults() {
        // Upstream `model-config` defaults (contextWindow 128000, maxTokens
        // 16384): a model WITHOUT the fields must still get them, else
        // `compaction_settings` (src/agent.rs) returns None and
        // auto-compaction is silently off.
        let d = tmpdir();
        std::fs::write(
            d.join("models.json"),
            r#"{"providers": {"p": {"api": "openai-completions", "models": [{"id": "m1"}]}}}"#,
        )
        .unwrap();
        let mj = load(&d).unwrap();
        let m = mj.model("p", "m1").expect("model m1");
        assert_eq!(m.context_window, Some(128_000));
        assert_eq!(m.max_tokens, Some(16_384));
    }

    #[test]
    fn model_without_id_is_schema_error() {
        let d = tmpdir();
        std::fs::write(
            d.join("models.json"),
            r#"{"providers": {"p": {"models": [{"name": "no id"}]}}}"#,
        )
        .unwrap();
        let e = load(&d).unwrap_err().to_string();
        assert!(e.contains("id"), "{e}");
    }

    #[test]
    fn owner_rules() {
        let d = tmpdir();
        std::fs::write(
            d.join("models.json"),
            r#"{
  "providers": {
    "newprov": { "baseUrl": "http://x", "api": "openai-completions",
      "models": [{ "id": "m1" }, { "id": "m2" }] },
    "anthro": { "baseUrl": "http://y", "api": "anthropic-messages",
      "models": [{ "id": "m1" }] },
    "openai": { "baseUrl": "http://proxy", "api": "openai-completions",
      "models": [{ "id": "gpt-4o" }, { "id": "proxy-only" }] }
  }
}"#,
        )
        .unwrap();
        let mj = load(&d).unwrap();
        // New provider, listed model, openai-completions → core-native.
        assert!(mj.owner("newprov", "m1").is_some());
        assert!(mj.owner("newprov", "m2").is_some());
        // Model not in the list.
        assert!(mj.owner("newprov", "nope").is_none());
        // Unknown provider.
        assert!(mj.owner("ghost", "m1").is_none());
        // Other api types: not core-native yet.
        assert!(mj.owner("anthro", "m1").is_none());
        // Bundled catalog wins for a builtin provider name + bundled id…
        assert!(mj.owner("openai", "gpt-4o").is_none());
        // …but a models.json-only id on a builtin name is core-native.
        assert!(mj.owner("openai", "proxy-only").is_some());
        // core_models carries exactly the owned entries.
        let cm = mj.core_models();
        let keys: Vec<String> = cm
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.id))
            .collect();
        assert_eq!(keys, vec!["newprov/m1", "newprov/m2", "openai/proxy-only"]);
    }

    #[test]
    fn resolve_template_upstream_semantics() {
        std::env::set_var("E3S1_TMPL_A", "alpha");
        assert_eq!(resolve_template("literal").unwrap(), "literal");
        assert_eq!(resolve_template("$E3S1_TMPL_A").unwrap(), "alpha");
        assert_eq!(resolve_template("${E3S1_TMPL_A}").unwrap(), "alpha");
        assert_eq!(
            resolve_template("pre-$E3S1_TMPL_A-post").unwrap(),
            "pre-alpha-post"
        );
        // Upstream `resolveConfigValue`: `$b` is an env reference (a bare
        // `$` before a valid name) — an unset var is a typed error.
        let e = resolve_template("a$E3S1_TMPL_UNSET2")
            .unwrap_err()
            .to_string();
        assert!(e.contains("E3S1_TMPL_UNSET2"), "{e}");
        assert_eq!(resolve_template("$$").unwrap(), "$");
        std::env::remove_var("E3S1_TMPL_A");
        let e = resolve_template("$E3S1_TMPL_UNSET")
            .unwrap_err()
            .to_string();
        assert!(e.contains("E3S1_TMPL_UNSET"), "{e}");
        assert!(!e.contains("$E3S1_TMPL_UNSET"), "no material: {e}");
    }

    #[test]
    fn build_driver_requires_base_and_key() {
        let d = tmpdir();
        let prov = Provider {
            id: "p".into(),
            base_url: None,
            api: Some("openai-completions".into()),
            api_key: Some("k".into()),
            models: vec![],
        };
        let e = build_driver(&prov, "m", &d).unwrap_err().to_string();
        assert!(e.contains("baseUrl"), "{e}");
        let anthro = Provider {
            id: "p".into(),
            base_url: Some("http://x".into()),
            api: Some("anthropic-messages".into()),
            api_key: Some("k".into()),
            models: vec![],
        };
        let e = build_driver(&anthro, "m", &d).unwrap_err().to_string();
        assert!(e.contains("not core-native yet"), "{e}");
        let cmd = Provider {
            base_url: Some("http://x".into()),
            api_key: Some("!cmd-key".into()),
            ..prov.clone()
        };
        let e = build_driver(&cmd, "m", &d).unwrap_err().to_string();
        assert!(e.contains("not supported"), "{e}");
        // No apiKey → the core credential store (auth.json) applies.
        std::fs::write(
            d.join("auth.json"),
            r#"{"p": {"type": "api_key", "key": "stored"}}"#,
        )
        .unwrap();
        let stored = Provider {
            base_url: Some("http://x".into()),
            api_key: None,
            ..prov.clone()
        };
        assert!(matches!(
            build_driver(&stored, "m", &d),
            Ok(ProviderDriver::OpenAi(_))
        ));
    }

    /// The core driver hits the provider baseUrl with the resolved key:
    /// $ENV_VAR interpolation, bearer auth, streaming response.
    #[tokio::test]
    async fn build_driver_streams_through_mock_with_env_key() {
        let d = tmpdir();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sse = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"mj-ok\"}}]}\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\ndata: [DONE]\n";
        let base =
            crate::provider::mock::start_mock(crate::provider::mock::sse_response(sse), tx).await;
        let prov = Provider {
            id: "mj".into(),
            base_url: Some(base),
            api: Some("openai-completions".into()),
            api_key: Some("$E3S1_MJ_KEY".into()),
            models: vec![],
        };
        std::env::set_var("E3S1_MJ_KEY", "mj-env-key");
        let drv = build_driver(&prov, "m1", &d).unwrap();
        let turn = drv
            .complete(
                &[serde_json::json!({"role": "user", "content": "hi"})],
                Box::new(|_: crate::model::StreamDelta| {}),
            )
            .await
            .expect("stream through the mock");
        assert_eq!(turn.text.as_deref(), Some("mj-ok"));
        std::env::remove_var("E3S1_MJ_KEY");
        let rec = rx.recv().await.expect("mock recorded the request");
        assert!(rec.contains("authorization: bearer mj-env-key"), "{rec}");
        assert!(rec.contains("/chat/completions"), "{rec}");
    }
}
