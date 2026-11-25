//! Model registry: the bundled upstream catalog (WP-302).
//!
//! Build time: `build.rs` merges the committed `catalog/data/*.json`
//! snapshot into a single JSON blob in OUT_DIR (zero runtime deps on the
//! upstream tree; D14 — the catalog is frozen at the committed snapshot,
//! refreshed only via the re-pin procedure in `catalog/README.md`).
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
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

/// The bundled catalog, parsed once and shared (the 1.1 MB JSON catalog
/// expands to several MB of heap — never hold a second copy, WP-1401
/// bench regression: a per-run clone doubled idle RSS).
fn bundled_shared() -> &'static Arc<ModelRegistry> {
    static ONCE: OnceLock<Arc<ModelRegistry>> = OnceLock::new();
    ONCE.get_or_init(|| {
        let raw: BTreeMap<String, Model> =
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/models.json"))).unwrap();
        Arc::new(ModelRegistry {
            by_key: raw.into_iter().collect(),
        })
    })
}

impl ModelRegistry {
    /// The bundled catalog, parsed once (1.1 MB JSON, a few ms).
    pub fn bundled() -> &'static ModelRegistry {
        bundled_shared().as_ref()
    }

    /// The bundled catalog as a shared `Arc` (WP-1401: the effective
    /// registry when no models.json extras exist — no clone).
    pub fn bundled_arc() -> Arc<ModelRegistry> {
        bundled_shared().clone()
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

    /// The catalog with extra models merged in (WP-1401/E3S1: models.json
    /// core-native models). An extra never replaces a bundled entry — the
    /// bundled catalog wins per model id (documented v0 limit, E2S1).
    /// Clones the whole map — only called when extras exist (the caller
    /// uses [`Self::bundled_arc`] otherwise, to keep a single catalog copy
    /// in memory).
    pub fn merged_with(&self, extra: &[Model]) -> ModelRegistry {
        let mut by_key = self.by_key.clone();
        for m in extra {
            by_key
                .entry(format!("{}/{}", m.provider, m.id))
                .or_insert(m.clone());
        }
        ModelRegistry { by_key }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir() -> std::path::PathBuf {
        // D14: the committed snapshot is the source of truth — the upstream
        // tree's data dir is regenerated from the live models.dev API and
        // may legitimately differ (that drift is the point of the freeze;
        // refreshing is the re-pin procedure in catalog/README.md).
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/data")
    }

    #[test]
    fn bundled_count_matches_committed_snapshot() {
        // Pins the bundled registry to the committed catalog/data snapshot
        // (catches merge-semantics divergence between build.rs and this
        // counting — e.g. the dedupe rule below). Unique `provider/id`
        // keys: upstream lists one model under several
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
            "catalog drift vs the committed snapshot (catalog/data)"
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
