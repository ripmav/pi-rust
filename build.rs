//! Merges the bundled provider catalog into a single JSON blob at build time.
//!
//! D14 (PO, 2026-11-24, Option 3): the catalog is FROZEN for pi-rust. The
//! committed `catalog/data/*.json` snapshot is the single source of truth
//! for the bundled registry — no live models.dev, no dependence on the local
//! upstream tree (its `packages/ai/src/providers/data/` is regenerated from
//! the live models.dev API on `npm run build`). Refreshing the snapshot when
//! new models are actually needed: see the re-pin procedure in
//! `catalog/README.md`.
//!
//! Byte stability: serde_json without `preserve_order` is BTreeMap-backed,
//! so every object (top level and each model entry) is emitted with sorted
//! keys — the same input files always yield the same bytes, regardless of
//! readdir order.
use serde_json::{Map, Value};
use sha2::Digest;
use std::path::{Path, PathBuf};

fn main() {
    let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog/data");
    println!("cargo:rerun-if-changed={}", data_dir.display());
    let mut files: Vec<PathBuf> = std::fs::read_dir(&data_dir)
        .unwrap_or_else(|e| {
            panic!(
                "bundled catalog dir {} missing ({e}): this repo ships it — the checkout is damaged; re-clone or restore catalog/ (see catalog/README.md)",
                data_dir.display()
            )
        })
        .map(|e| e.expect("reading bundled catalog dir").path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            !name.starts_with('.') && p.extension().is_some_and(|e| e == "json")
        })
        .collect();
    files.sort();
    // Fingerprint of the committed inputs (name-sorted `name\0content`
    // stream). build.rs stamps it next to the baked catalog; the CI
    // catalog-verify guard recomputes it from the checkout and drops stale
    // restored caches on a content (not mtime) mismatch — the mtime race
    // behind the WP-1403 incident.
    let mut inputs = sha2::Sha256::new();
    let mut merged = Map::new();
    for path in &files {
        let raw = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        inputs.update(path.file_name().unwrap().to_string_lossy().as_bytes());
        inputs.update(b"\0");
        inputs.update(&raw);
        let obj: Value = serde_json::from_slice(&raw)
            .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
        let provider = path.file_stem().unwrap().to_string_lossy().to_string();
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
    // Sanity guard (WP-1403 CI incident, 2026-11-23, kept as paranoia per
    // D14): a truncated or partial snapshot would silently produce a
    // shrunk catalog, which later surfaces as confusing "model not in the
    // registry" test failures. The frozen catalog has ~1.8k entries; any
    // file loss of significance drops it well below this.
    if merged.len() < 1000 {
        panic!(
            "bundled model catalog has only {} entries (expected ~1800) — catalog/data looks incomplete; do not commit a partial snapshot (re-pin procedure: catalog/README.md)", merged.len()
        );
    }
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("models.json");
    std::fs::write(&out, serde_json::to_string(&Value::Object(merged)).unwrap()).unwrap();
    let fingerprint = {
        let digest = inputs.finalize();
        let mut s = String::new();
        for b in digest.iter() {
            s.push_str(&format!("{b:02x}"));
        }
        s
    };
    std::fs::write(out.with_file_name("catalog-inputs.sha256"), &fingerprint).unwrap();
}
