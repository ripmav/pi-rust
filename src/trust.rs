//! WP-1102: project trust model — parity port of the pinned upstream
//! `core/trust-manager.ts` + `core/project-trust.ts` (decisions D1–D8 in
//! `roadmap/m11-v1-project-trust-and-sealing/wp-1102-project-trust-model.md`).
//!
//! - **Store (D1):** `<agentDir>/trust.json` =
//!   `{ "<canonical-path>": true | false | null }`. Lookup = the nearest
//!   explicit decision, walking up from cwd to the filesystem root; `null` =
//!   explicitly cleared. Deviation: no file lock (upstream uses
//!   proper-lockfile) — pi-rust is single-process, so there is no
//!   concurrent writer to guard against.
//! - **Trust-requiring resources (D2):** pi-rust surface only — `settings.json`,
//!   `mcp.json`, `skills`, `prompts`, `extensions` under `<cwd>/.pi/` (D2,
//!   revised: every project surface pi-rust loads). Upstream's `themes`,
//!   `SYSTEM.md`, `APPEND_SYSTEM.md`, `.agents/skills`
//!   are re-evaluated as triggers when those surfaces land in pi-rust.
//! - **Resolution (D3):** upstream's `project_trust` extension event step is
//!   skipped — documented at [`resolve_project_trusted`].

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One stored trust decision. `None` = explicitly cleared (JSON `null`).
pub type TrustDecision = Option<bool>;

/// The nearest explicit decision found for a cwd (path + value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    pub path: String,
    pub decision: bool,
}

/// One store update: the target path (canonicalized on write) and the
/// decision to store (`None` clears any existing entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustUpdate {
    pub path: PathBuf,
    pub decision: TrustDecision,
}

/// One trust-prompt option (upstream `ProjectTrustOption`), labels verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustOption {
    pub label: String,
    pub trusted: bool,
    /// Store updates applied when this option is chosen (session-only
    /// options write nothing).
    pub updates: Vec<TrustUpdate>,
    /// The path this option's decision refers to (the saved-decision
    /// marker); `None` for session-only options.
    pub saved_path: Option<String>,
}

/// Upstream `normalizeCwd`: `canonicalizePath(resolvePath(cwd))` — absolute
/// with symlinks resolved; a path that cannot be resolved (does not exist)
/// falls back to its absolute form (upstream `canonicalizePath` swallows
/// realpath errors).
fn normalize_path(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    std::fs::canonicalize(&abs).unwrap_or(abs)
}

/// Upstream `pathsEqual`: case-folded on Windows (NTFS is
/// case-insensitive), byte-exact elsewhere.
fn paths_equal(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Upstream `getProjectTrustParentPath`: the parent of the normalized cwd,
/// `None` at the filesystem root.
pub fn trust_parent_path(cwd: &Path) -> Option<PathBuf> {
    let t = normalize_path(cwd);
    t.parent().filter(|p| *p != t).map(PathBuf::from)
}

/// Upstream `getProjectTrustOptions` (same order, labels verbatim).
pub fn trust_options(cwd: &Path, include_session_only: bool) -> Vec<TrustOption> {
    let trust_path = normalize_path(cwd);
    let mut options = vec![TrustOption {
        label: "Trust".into(),
        trusted: true,
        updates: vec![TrustUpdate {
            path: trust_path.clone(),
            decision: Some(true),
        }],
        saved_path: Some(trust_path.to_string_lossy().into_owned()),
    }];
    if let Some(parent) = trust_parent_path(&trust_path) {
        options.push(TrustOption {
            label: format!("Trust parent folder ({})", parent.display()),
            trusted: true,
            updates: vec![
                TrustUpdate {
                    path: parent.clone(),
                    decision: Some(true),
                },
                TrustUpdate {
                    path: trust_path.clone(),
                    decision: None,
                },
            ],
            saved_path: Some(parent.to_string_lossy().into_owned()),
        });
    }
    if include_session_only {
        options.push(TrustOption {
            label: "Trust (this session only)".into(),
            trusted: true,
            updates: vec![],
            saved_path: None,
        });
    }
    options.push(TrustOption {
        label: "Do not trust".into(),
        trusted: false,
        updates: vec![TrustUpdate {
            path: trust_path.clone(),
            decision: Some(false),
        }],
        saved_path: Some(trust_path.to_string_lossy().into_owned()),
    });
    if include_session_only {
        options.push(TrustOption {
            label: "Do not trust (this session only)".into(),
            trusted: false,
            updates: vec![],
            saved_path: None,
        });
    }
    options
}

/// Upstream `readTrustFile` (messages verbatim): absent file = empty store;
/// malformed JSON / non-object / non-boolean-null values are typed errors
/// naming the path.
fn read_trust_file(path: &Path) -> Result<BTreeMap<String, TrustDecision>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading trust store {}", path.display()))?;
    // Upstream stripBom: a BOM would otherwise reach the JSON parser.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let parsed: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("Failed to read trust store {}: {e}", path.display()))?;
    let obj = parsed.as_object().ok_or_else(|| {
        anyhow::anyhow!("Invalid trust store {}: expected an object", path.display())
    })?;
    let mut data = BTreeMap::new();
    for (key, value) in obj {
        let decision = match value {
            serde_json::Value::Bool(b) => Some(*b),
            serde_json::Value::Null => None,
            _ => bail!(
                "Invalid trust store {}: value for \"{}\" must be true, false, or null",
                path.display(),
                key
            ),
        };
        data.insert(key.clone(), decision);
    }
    Ok(data)
}

/// Upstream `writeTrustFile`: keys sorted, pretty-printed (2 spaces) with a
/// trailing newline.
fn write_trust_file(path: &Path, data: &BTreeMap<String, TrustDecision>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating trust store parent {}", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(data)?;
    std::fs::write(path, format!("{body}\n"))
        .with_context(|| format!("writing trust store {}", path.display()))?;
    Ok(())
}

/// Upstream `findNearestTrustEntry`: nearest explicit decision walking up
/// from cwd to the root; `null` entries are not decisions (they clear).
fn find_nearest_entry(data: &BTreeMap<String, TrustDecision>, cwd: &Path) -> Option<TrustEntry> {
    let mut current = normalize_path(cwd);
    loop {
        for (key, value) in data {
            if paths_equal(key, &current.to_string_lossy()) {
                if let Some(decision) = value {
                    return Some(TrustEntry {
                        path: key.clone(),
                        decision: *decision,
                    });
                }
            }
        }
        match current.parent() {
            Some(parent) if parent != current.as_path() => current = parent.to_path_buf(),
            _ => return None, // filesystem root (or a degenerate self-parent)
        }
    }
}

/// Upstream `ProjectTrustStore`: `<agentDir>/trust.json`.
pub struct ProjectTrustStore {
    path: PathBuf,
}

impl ProjectTrustStore {
    pub fn new(agent_dir: &Path) -> Self {
        Self {
            path: agent_dir.join("trust.json"),
        }
    }

    /// The store file (`<agentDir>/trust.json`).
    /// Upstream API surface (the tests + WP-1103 consumers use it);
    /// allow until a production caller exists.
    #[allow(dead_code)]
    pub fn trust_path(&self) -> &Path {
        &self.path
    }

    /// The nearest explicit decision (`None` = nothing stored).
    pub fn get(&self, cwd: &Path) -> Result<TrustDecision> {
        Ok(self.get_entry(cwd)?.map(|e| e.decision))
    }

    /// The nearest explicit entry (path + decision), or `None`.
    pub fn get_entry(&self, cwd: &Path) -> Result<Option<TrustEntry>> {
        Ok(find_nearest_entry(&read_trust_file(&self.path)?, cwd))
    }

    /// Store one decision for a path (`None` clears).
    /// Upstream `set` (single-decision convenience over `setMany`);
    /// allow until a production caller exists.
    #[allow(dead_code)]
    pub fn set(&self, path: &Path, decision: TrustDecision) -> Result<()> {
        self.set_many(&[TrustUpdate {
            path: path.to_path_buf(),
            decision,
        }])
    }

    /// Store several decisions (upstream `setMany`: later updates win,
    /// existing case-variant keys are replaced, `None` removes the key).
    pub fn set_many(&self, updates: &[TrustUpdate]) -> Result<()> {
        let mut data = read_trust_file(&self.path)?;
        for u in updates {
            let key = normalize_path(&u.path).to_string_lossy().into_owned();
            let stale: Vec<String> = data
                .keys()
                .filter(|k| paths_equal(k, &key))
                .cloned()
                .collect();
            for k in stale {
                data.remove(&k);
            }
            if u.decision.is_some() {
                data.insert(key, u.decision);
            }
        }
        write_trust_file(&self.path, &data)
    }
}

/// Upstream `hasTrustRequiringProjectResources` for the pi-rust surface
/// (D2): true when `<cwd>/.pi/` holds `settings.json`, `mcp.json` or a
/// `skills` entry. Nothing else (upstream's `extensions`, `prompts`,
/// `themes`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `.agents/skills` are not
/// trust triggers in pi-rust).
pub fn has_trust_requiring_resources(cwd: &Path) -> bool {
    let config_dir = cwd.join(".pi");
    // D2 (revised 2026-11-17): every project surface pi-rust loads —
    // settings/mcp (WP-1103 merge), skills, prompts (templates.rs),
    // extensions (sidecar discovery). Upstream's `themes`, `SYSTEM.md`,
    // `APPEND_SYSTEM.md`, `.agents/skills` are not loaded by pi-rust.
    [
        "settings.json",
        "mcp.json",
        "skills",
        "prompts",
        "extensions",
    ]
    .iter()
    .any(|entry| config_dir.join(entry).exists())
}

/// The outcome of [`resolve_project_trusted`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustResolution {
    /// Resolution completed: the effective trust value.
    Resolved(bool),
    /// Resolution reached the interactive "ask" step (`has_ui` = true, no
    /// override/stored decision, `defaultProjectTrust` = ask): the caller
    /// shows the prompt and applies the chosen option.
    Ask,
}

/// Upstream `resolveProjectTrusted` (project-trust.ts), in order:
/// 1. CLI override (`--approve`/`-a` = true, `--no-approve`/`-na` = false).
/// 2. No trust-requiring resources in cwd → stored decision ?? true
///    (clean dirs never prompt).
/// 3. **D3: no `project_trust` extension event in v0** — project
///    extensions load (via the sidecar) only *after* resolution and only
///    when trusted, so no untrusted project-extension handler can answer
///    the decision itself. The upstream event mechanism arrives with full
///    extension-event parity.
/// 4. Stored decision (nearest ancestor) → return it.
/// 5. `defaultProjectTrust`: `"always"` → true, `"never"` → false,
///    `"ask"` (default) → prompt.
/// 6. No UI (print/json/rpc) → **false** (not trusted; D7).
/// 7. Interactive → [`TrustResolution::Ask`] (the caller prompts).
pub fn resolve_project_trusted(
    cwd: &Path,
    store: &ProjectTrustStore,
    cli_override: Option<bool>,
    default_project_trust: Option<&str>,
    has_ui: bool,
) -> Result<TrustResolution> {
    if let Some(override_) = cli_override {
        return Ok(TrustResolution::Resolved(override_));
    }
    if !has_trust_requiring_resources(cwd) {
        return Ok(TrustResolution::Resolved(store.get(cwd)?.unwrap_or(true)));
    }
    // (step 3: the `project_trust` extension event — D3, documented above.)
    if let Some(decision) = store.get(cwd)? {
        return Ok(TrustResolution::Resolved(decision));
    }
    match default_project_trust.unwrap_or("ask") {
        "always" => return Ok(TrustResolution::Resolved(true)),
        "never" => return Ok(TrustResolution::Resolved(false)),
        "ask" => {}
        other => bail!("invalid defaultProjectTrust value '{other}' (valid: ask, always, never)"),
    }
    if !has_ui {
        return Ok(TrustResolution::Resolved(false));
    }
    Ok(TrustResolution::Ask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-trust-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A tree: root/a, root/a/b, root/a/sib.
    fn tree() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = tmpdir();
        let a = root.join("a");
        let b = a.join("b");
        let sib = a.join("sib");
        for d in [&root, &a, &b, &sib] {
            std::fs::create_dir_all(d).unwrap();
        }
        (root, a, b, sib)
    }

    /// Same normalization the implementation applies (canonicalize with
    /// fallback): expected values must use this, never the raw tmp path
    /// (macOS /tmp -> /private/tmp symlink; Windows case folding).
    fn canon(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
    }

    // --- store: set/get, nearest-ancestor, null clears, canonicalization ---

    #[test]
    fn store_set_get_roundtrip() {
        let d = tmpdir();
        let store = ProjectTrustStore::new(&d);
        let cwd = d.join("proj");
        std::fs::create_dir_all(&cwd).unwrap();
        assert_eq!(store.get(&cwd).unwrap(), None, "missing file = no entry");
        store.set(&cwd, Some(true)).unwrap();
        assert_eq!(store.get(&cwd).unwrap(), Some(true));
        store.set(&cwd, Some(false)).unwrap();
        assert_eq!(store.get(&cwd).unwrap(), Some(false));
    }

    #[test]
    fn store_nearest_ancestor_wins() {
        let (root, a, b, sib) = tree();
        let store = ProjectTrustStore::new(&root);
        store.set(&a, Some(false)).unwrap();
        store.set(&b, Some(true)).unwrap();
        let deep = b.join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        // Nearest explicit decision: b=true for b and everything below it.
        assert_eq!(store.get(&b).unwrap(), Some(true));
        assert_eq!(store.get(&deep).unwrap(), Some(true));
        // a's own decision applies outside b's subtree.
        assert_eq!(store.get(&a).unwrap(), Some(false));
        assert_eq!(store.get(&sib).unwrap(), Some(false));
        // Above the topmost entry: nothing stored.
        assert_eq!(store.get(&root).unwrap(), None);
        // get_entry carries the deciding path (upstream ProjectTrustStoreEntry).
        let e = store.get_entry(&deep).unwrap().unwrap();
        assert!(e.decision);
        assert_eq!(Path::new(&e.path), canon(&b), "{e:?}");
    }

    #[test]
    fn store_null_clears_and_falls_back_to_ancestor() {
        let (_root, a, b, _sib) = tree();
        let store = ProjectTrustStore::new(&a);
        store.set(&a, Some(true)).unwrap();
        store.set(&b, Some(false)).unwrap();
        assert_eq!(store.get(&b).unwrap(), Some(false));
        // Explicitly clear b: upstream setMany removes the key, so the
        // nearest decision becomes a's again.
        store.set(&b, None).unwrap();
        assert_eq!(store.get(&b).unwrap(), Some(true));
        // The cleared key is gone from the file (upstream shape: null is
        // a clearing value, not a stored one).
        let raw = std::fs::read_to_string(store.trust_path()).unwrap();
        assert!(!raw.contains(canon(&b).to_string_lossy().as_ref()), "{raw}");
        assert!(raw.contains(canon(&a).to_string_lossy().as_ref()), "{raw}");
        // A later set after a clear replaces cleanly.
        store.set(&b, Some(true)).unwrap();
        assert_eq!(store.get(&b).unwrap(), Some(true));
    }

    #[test]
    fn store_path_canonicalization() {
        let d = tmpdir();
        let proj = d.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let store = ProjectTrustStore::new(&d);
        // Trailing slashes normalize to the same key.
        store.set(&proj, Some(true)).unwrap();
        store
            .set(Path::new(&format!("{}/", proj.display())), Some(false))
            .unwrap();
        assert_eq!(store.get(&proj).unwrap(), Some(false));
        let raw = std::fs::read_to_string(store.trust_path()).unwrap();
        // Both writes hit the one canonical key (no duplicate entries).
        let key = canon(&proj).to_string_lossy().to_string();
        assert_eq!(raw.matches(key.as_str()).count(), 1, "{raw}");
    }

    #[cfg(unix)]
    #[test]
    fn store_path_canonicalization_follows_symlinks() {
        use std::os::unix::fs::symlink;
        let d = tmpdir();
        let proj = d.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let link = d.join("link");
        symlink(&proj, &link).unwrap();
        let store = ProjectTrustStore::new(&d);
        store.set(&link, Some(true)).unwrap();
        // The lookup via the real path finds the entry (both canonicalize).
        assert_eq!(store.get(&proj).unwrap(), Some(true));
    }

    #[test]
    fn store_malformed_shapes_error() {
        let d = tmpdir();
        let store = ProjectTrustStore::new(&d);
        let p = d.join("proj");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(store.trust_path(), "{not json").unwrap();
        let err = store.get(&p).unwrap_err().to_string();
        assert!(err.contains("Failed to read trust store"), "{err}");
        assert!(err.contains("trust.json"), "{err}");
        std::fs::write(store.trust_path(), "[1,2]").unwrap();
        let err = store.get(&p).unwrap_err().to_string();
        assert!(err.contains("expected an object"), "{err}");
        std::fs::write(store.trust_path(), r#"{"a":1}"#).unwrap();
        let err = store.get(&p).unwrap_err().to_string();
        assert!(
            err.contains("value for \"a\" must be true, false, or null"),
            "{err}"
        );
    }

    #[test]
    fn store_set_many_parent_option_shape() {
        // "Trust parent folder" = parent true + child cleared (key removed;
        // the child's own entry no longer shadows the parent's decision).
        let (_root, a, b, _sib) = tree();
        let store = ProjectTrustStore::new(&a);
        store
            .set_many(&[
                TrustUpdate {
                    path: b.clone(),
                    decision: Some(false),
                },
                TrustUpdate {
                    path: a.clone(),
                    decision: Some(true),
                },
                TrustUpdate {
                    path: b.clone(),
                    decision: None,
                },
            ])
            .unwrap();
        assert_eq!(store.get(&a).unwrap(), Some(true));
        // b's key was cleared: the lookup falls through to a's decision.
        assert_eq!(
            store.get(&b).unwrap(),
            Some(true),
            "cleared entry falls back to the ancestor"
        );
        let raw = std::fs::read_to_string(store.trust_path()).unwrap();
        assert!(
            !raw.contains(canon(&b).to_string_lossy().as_ref()),
            "cleared key is removed: {raw}"
        );
        assert!(
            raw.contains(canon(&a).to_string_lossy().as_ref()),
            "parent key stored canonical: {raw}"
        );
    }

    // --- trust_options: upstream order/labels/updates ---

    #[test]
    fn options_order_labels_and_updates() {
        let (_root, a, b, _sib) = tree();
        let (a, b) = (canon(&a), canon(&b));
        let opts = trust_options(&b, true);
        assert_eq!(opts.len(), 5);
        assert_eq!(
            opts.iter().map(|o| o.label.clone()).collect::<Vec<_>>(),
            vec![
                "Trust".to_string(),
                format!("Trust parent folder ({})", a.display()),
                "Trust (this session only)".to_string(),
                "Do not trust".to_string(),
                "Do not trust (this session only)".to_string(),
            ]
        );
        assert!(opts[0].trusted);
        assert_eq!(
            opts[0].updates,
            vec![TrustUpdate {
                path: b.clone(),
                decision: Some(true)
            }]
        );
        assert_eq!(
            opts[0].saved_path.as_deref(),
            Some(b.to_string_lossy().as_ref())
        );
        // Parent option: parent trusted + cwd explicitly cleared.
        assert!(opts[1].trusted);
        assert_eq!(
            opts[1].updates,
            vec![
                TrustUpdate {
                    path: a.clone(),
                    decision: Some(true)
                },
                TrustUpdate {
                    path: b.clone(),
                    decision: None
                },
            ]
        );
        assert_eq!(
            opts[1].saved_path.as_deref(),
            Some(a.to_string_lossy().as_ref())
        );
        // Session-only options write nothing.
        assert!(opts[2].trusted);
        assert!(opts[2].updates.is_empty());
        assert_eq!(opts[2].saved_path, None);
        assert!(!opts[3].trusted);
        assert_eq!(
            opts[3].updates,
            vec![TrustUpdate {
                path: b.clone(),
                decision: Some(false)
            }]
        );
        assert!(!opts[4].trusted);
        assert!(opts[4].updates.is_empty());
    }

    #[test]
    fn options_without_session_only() {
        let (_root, _a, b, _sib) = tree();
        let opts = trust_options(&b, false);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0].label, "Trust");
        assert_eq!(
            opts[1].label,
            format!(
                "Trust parent folder ({})",
                canon(b.parent().unwrap()).display()
            )
        );
        assert_eq!(opts[2].label, "Do not trust");
    }

    #[test]
    fn options_at_fs_root_have_no_parent() {
        let opts = trust_options(Path::new("/"), true);
        assert_eq!(opts.len(), 4);
        assert!(!opts
            .iter()
            .any(|o| o.label.starts_with("Trust parent folder")));
    }

    // --- has_trust_requiring_resources: D2 surface only ---

    #[test]
    fn resources_empty_dir_false() {
        let d = tmpdir();
        assert!(!has_trust_requiring_resources(&d));
        std::fs::create_dir_all(d.join(".pi")).unwrap();
        assert!(
            !has_trust_requiring_resources(&d),
            "empty .pi is not a trigger"
        );
    }

    #[test]
    fn resources_each_trigger_true() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join(".pi")).unwrap();
        std::fs::write(d.join(".pi/settings.json"), "{}").unwrap();
        assert!(has_trust_requiring_resources(&d));
        std::fs::remove_file(d.join(".pi/settings.json")).unwrap();
        std::fs::write(d.join(".pi/mcp.json"), "{}").unwrap();
        assert!(has_trust_requiring_resources(&d));
        std::fs::remove_file(d.join(".pi/mcp.json")).unwrap();
        std::fs::create_dir_all(d.join(".pi/skills")).unwrap();
        assert!(has_trust_requiring_resources(&d));
        std::fs::remove_dir_all(d.join(".pi/skills")).unwrap();
        std::fs::create_dir_all(d.join(".pi/prompts")).unwrap();
        assert!(has_trust_requiring_resources(&d));
        std::fs::remove_dir_all(d.join(".pi/prompts")).unwrap();
        std::fs::create_dir_all(d.join(".pi/extensions")).unwrap();
        assert!(has_trust_requiring_resources(&d));
    }

    #[test]
    fn resources_unrelated_files_false() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join(".pi")).unwrap();
        std::fs::write(d.join(".pi/SYSTEM.md"), "x").unwrap();
        std::fs::write(d.join(".pi/other.txt"), "x").unwrap();
        std::fs::create_dir_all(d.join(".agents/skills")).unwrap();
        assert!(
            !has_trust_requiring_resources(&d),
            "D2: pi-rust surface only"
        );
    }

    // --- resolve_project_trusted: upstream order ---

    fn store_in(d: &Path) -> ProjectTrustStore {
        ProjectTrustStore::new(d)
    }

    fn cwd_with_resources(d: &Path) -> PathBuf {
        let cwd = d.join("proj");
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::write(cwd.join(".pi/settings.json"), "{}").unwrap();
        cwd
    }

    #[test]
    fn resolve_override_wins() {
        let d = tmpdir();
        let cwd = cwd_with_resources(&d);
        let store = store_in(&d);
        store.set(&cwd, Some(false)).unwrap();
        assert_eq!(
            resolve_project_trusted(&cwd, &store, Some(true), None, false).unwrap(),
            TrustResolution::Resolved(true),
            "--approve beats a stored untrusted decision"
        );
        store.set(&cwd, Some(true)).unwrap();
        assert_eq!(
            resolve_project_trusted(&cwd, &store, Some(false), None, true).unwrap(),
            TrustResolution::Resolved(false),
            "--no-approve beats a stored trusted decision"
        );
    }

    #[test]
    fn resolve_no_resources_stored_or_true() {
        let d = tmpdir();
        let cwd = d.join("clean");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = store_in(&d);
        // Nothing stored: clean dirs are trusted without prompting.
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("ask"), true).unwrap(),
            TrustResolution::Resolved(true)
        );
        // A stored decision is returned (upstream `?? true`).
        store.set(&cwd, Some(false)).unwrap();
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("always"), false).unwrap(),
            TrustResolution::Resolved(false)
        );
    }

    #[test]
    fn resolve_stored_decision_returned() {
        let d = tmpdir();
        let cwd = cwd_with_resources(&d);
        let store = store_in(&d);
        store.set(&cwd, Some(false)).unwrap();
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("always"), true).unwrap(),
            TrustResolution::Resolved(false),
            "stored decision beats defaultProjectTrust"
        );
        store.set(&cwd, Some(true)).unwrap();
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("never"), false).unwrap(),
            TrustResolution::Resolved(true),
        );
    }

    #[test]
    fn resolve_default_always_never_ask_headless() {
        let d = tmpdir();
        let cwd = cwd_with_resources(&d);
        let store = store_in(&d);
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("always"), false).unwrap(),
            TrustResolution::Resolved(true)
        );
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("never"), false).unwrap(),
            TrustResolution::Resolved(false)
        );
        // "ask" (or no default) without a UI: not trusted (D7).
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("ask"), false).unwrap(),
            TrustResolution::Resolved(false)
        );
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, None, false).unwrap(),
            TrustResolution::Resolved(false)
        );
    }

    #[test]
    fn resolve_ask_reaches_prompt() {
        let d = tmpdir();
        let cwd = cwd_with_resources(&d);
        let store = store_in(&d);
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, Some("ask"), true).unwrap(),
            TrustResolution::Ask
        );
        assert_eq!(
            resolve_project_trusted(&cwd, &store, None, None, true).unwrap(),
            TrustResolution::Ask,
            "no default = ask"
        );
    }

    #[test]
    fn resolve_invalid_default_errors() {
        let d = tmpdir();
        let cwd = cwd_with_resources(&d);
        let store = store_in(&d);
        let err = resolve_project_trusted(&cwd, &store, None, Some("sometimes"), false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("defaultProjectTrust"), "{err}");
    }
}
