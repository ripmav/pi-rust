//! Model registry: the bundled upstream catalog (WP-302).
//!
//! Build time: `build.rs` merges `upstream/packages/ai/src/providers/data/*.json`
//! into a single JSON blob in OUT_DIR (zero runtime deps on the upstream
//! tree — the catalog is frozen at build time, matching the upstream pin).
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub api: String,
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub cost: Cost,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Kept raw for now; consumed by WP-406 (thinking levels).
    #[serde(default)]
    #[serde(rename = "thinkingLevelMap")]
    pub thinking_level_map: Option<serde_json::Value>,
}

/// Resolution key: `provider/modelId`.
pub type ModelKey = String;

pub struct ModelRegistry {
    by_key: BTreeMap<ModelKey, Model>,
}

impl ModelRegistry {
    /// The bundled catalog, parsed once (1.1 MB JSON, a few ms).
    pub fn bundled() -> &'static ModelRegistry {
        static ONCE: OnceLock<ModelRegistry> = OnceLock::new();
        ONCE.get_or_init(|| {
            let raw: BTreeMap<String, Model> =
                serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/models.json")))
                    .unwrap();
            let by_key = raw.into_iter().collect();
            ModelRegistry { by_key }
        })
    }

    /// Read by the in-crate tests (bundled count).
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.by_key.len()
    }

    pub fn resolve(&self, provider: &str, id: &str) -> Option<&Model> {
        self.by_key.get(&format!("{provider}/{id}"))
    }

    /// Every distinct provider in the catalog, sorted.
    pub fn providers(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.by_key.values().map(|m| m.provider.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn models_of(&self, provider: &str) -> Vec<&Model> {
        self.by_key
            .values()
            .filter(|m| m.provider == provider)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("upstream/packages/ai/src/providers/data")
    }

    #[test]
    fn bundled_count_matches_upstream_data() {
        // Unique `provider/id` keys: upstream lists one model under several
        // api groups (e.g. openai-completions + openai-responses); the
        // registry dedupes per provider/id (api variants matter only for
        // driver dispatch, M4).
        let mut keys = std::collections::BTreeSet::new();
        for f in std::fs::read_dir(data_dir())
            .unwrap()
            .filter_map(|e| e.ok())
        {
            let name = f.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && f.path().extension().is_some_and(|e| e == "json") {
                let provider = name.trim_end_matches(".json").to_string();
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(f.path()).unwrap()).unwrap();
                for models in v.as_object().unwrap().values() {
                    for e in models.as_object().unwrap().values() {
                        let id = e
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        keys.insert(format!("{provider}/{id}"));
                    }
                }
            }
        }
        assert_eq!(
            ModelRegistry::bundled().count(),
            keys.len(),
            "catalog drift vs upstream data dir"
        );
    }

    #[test]
    fn resolves_anthropic_entry_with_fields() {
        let m = ModelRegistry::bundled()
            .resolve("anthropic", "claude-fable-5")
            .expect("claude-fable-5");
        assert_eq!(m.context_window, Some(1000000));
        assert_eq!(m.max_tokens, Some(128000));
        assert!(m.reasoning);
        assert_eq!(m.cost.input, 10.0);
        assert_eq!(m.input, vec!["text", "image"]);
    }

    #[test]
    fn resolves_groq_compat_entry() {
        let m = ModelRegistry::bundled()
            .resolve("groq", "llama-3.1-8b-instant")
            .expect("llama-3.1-8b-instant");
        assert_eq!(m.context_window, Some(131072));
        assert!(!m.reasoning);
    }

    #[test]
    fn unknown_model_is_none_and_provider_list_sane() {
        let r = ModelRegistry::bundled();
        assert!(r.resolve("anthropic", "no-such-model").is_none());
        let provs = r.providers();
        assert!(
            provs.contains(&"anthropic") && provs.contains(&"openai") && provs.contains(&"groq")
        );
        assert!(!r.models_of("anthropic").is_empty());
    }
}
