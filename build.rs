//! Merges the upstream provider catalog data into a single JSON blob at
//! build time (frozen to the upstream pin; no runtime dependency on the
//! upstream tree for the registry).
use serde_json::{Map, Value};
use std::path::Path;

fn main() {
    let data_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("upstream/packages/ai/src/providers/data");
    println!("cargo:rerun-if-changed={}", data_dir.display());
    let mut merged = Map::new();
    for entry in std::fs::read_dir(&data_dir)
        .expect("upstream model data dir missing — clone upstream first (see README)")
    {
        let entry = entry.expect("reading upstream model data dir");
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || path.extension().is_some_and(|e| e != "json")
        {
            continue; // .manifest.json is metadata, not a catalog
        }
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let obj: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
        let provider = entry
            .path()
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        for models in obj
            .as_object()
            .expect("catalog file must be an object")
            .values()
        {
            for (key, model) in models.as_object().expect("catalog group must be an object") {
                let mut m = model.clone();
                m.as_object_mut()
                    .expect("model entry object")
                    .entry("provider")
                    .or_insert_with(|| Value::String(provider.clone()));
                let model_id = m.get("id").and_then(Value::as_str).unwrap_or(key);
                merged.insert(format!("{provider}/{model_id}"), m);
            }
        }
    }
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("models.json");
    std::fs::write(&out, serde_json::to_string(&Value::Object(merged)).unwrap()).unwrap();
}
