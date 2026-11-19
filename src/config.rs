//! Agent config from `<agent-dir>/settings.json` (WP-301).
//!
//! Upstream reads the global settings file from the agent dir; project
//! `cwd/.pi/settings.json` is merged in for trusted projects (WP-1103,
//! `merge_project_settings`) — untrusted projects never read the project
//! file at all. Project MCP is merged the same way (`merge_project_mcp`).
//!
//! Unknown keys are ignored (forward compat). Missing file = all defaults.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// Settings `defaultProvider` (e.g. "openai").
    pub default_provider: Option<String>,
    /// Settings `defaultModel` — bare id or `provider/model` form.
    pub default_model: Option<String>,
    /// Settings `defaultThinkingLevel`.
    pub default_thinking_level: Option<String>,
    /// Settings `sessionDir` (consumed by WP-304/305).
    pub session_dir: Option<String>,
    /// Settings `defaultTools` (consumed by M5).
    pub default_tools: Option<String>,
    /// Settings `theme` (consumed by M6 WP-607): a theme name, or a
    /// `light/dark` auto pair (v0 takes the dark side).
    pub theme: Option<String>,
    /// Settings `defaultProjectTrust` (WP-1102, D4): `"ask"` (default) |
    /// `"always"` | `"never"` — the fallback when a project with trust-
    /// requiring resources has no stored decision. Unknown values are an
    /// error naming the path (consistent with the other settings errors).
    pub default_project_trust: Option<String>,
}

/// Loads `<agent_dir>/settings.json`; absent file → defaults, malformed
/// file → error naming the path (exit 1 upstream of the agent loop).
pub fn load(agent_dir: &Path) -> Result<AgentConfig> {
    let path = agent_dir.join("settings.json");
    if !path.exists() {
        return Ok(AgentConfig::default());
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: AgentConfig = serde_json::from_str(&raw)
        .with_context(|| format!("parsing agent settings {}", path.display()))?;
    // WP-1102 (D4): closed value set, validated here (unknown value → error
    // naming the path, same rule as the other settings errors).
    if let Some(v) = &parsed.default_project_trust {
        if !matches!(v.as_str(), "ask" | "always" | "never") {
            bail!(
                "invalid defaultProjectTrust value '{v}' in {} (valid: ask, always, never)",
                path.display()
            );
        }
    }
    Ok(parsed)
}

/// Resolves the effective (provider, model) from CLI flags + settings:
/// - explicit `--provider` + `--model` → as-is
/// - explicit `--provider`, no `--model` → settings `defaultModel`
/// - no `--provider`: settings `defaultModel` in `provider/model` form → split
/// - no provider/model anywhere → ("fake", None) — scripted scenario mode
///
/// Bare `defaultModel` without any provider is an error: full ModelReference
/// resolution (bare id → per-provider default) arrives with the model
/// registry (WP-302).
pub fn resolve_provider_flags(
    provider: Option<&str>,
    model: Option<&str>,
    cfg: &AgentConfig,
) -> Result<(String, Option<String>)> {
    match (provider, model) {
		(Some(p), m) => Ok((p.to_string(), m.map(str::to_string).or_else(|| cfg.default_model.clone()))),
		(None, Some(_)) => bail!("--model without --provider is only valid with the fake driver; use --provider <p> or a `provider/model` defaultModel"),
		(None, None) => match &cfg.default_model {
			Some(dm) if dm.contains('/') => {
				let (p, m) = dm.split_once('/').unwrap();
				Ok((p.to_string(), Some(m.to_string())))
			}
			Some(dm) => {
				match &cfg.default_provider {
					Some(p) => Ok((p.clone(), Some(dm.clone()))),
					None => bail!("settings defaultModel {dm:?} needs `provider/model` form (or a settings defaultProvider); full model-id resolution arrives with the model registry (WP-302)"),
				}
			}
			None => Ok(("fake".into(), None)),
		},
	}
}

/// Loads `<agent_dir>/mcp.json` (WP-702/702c) for the sidecar init payload.
/// Upstream shape `{"mcpServers": {...}}` (the file `loadMcpConfig` reads);
/// absent file → `Ok(None)` (no MCP servers). Malformed file, a non-object
/// top level, or a non-object `mcpServers` → error naming the path (same
/// rule as settings.json). The project `<cwd>/.pi/mcp.json` merge (trusted
/// only) is WP-1103 (`merge_project_mcp`).
pub fn load_mcp_config(agent_dir: &Path) -> Result<Option<Value>> {
    let path = agent_dir.join("mcp.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing agent mcp config {}", path.display()))?;
    if !parsed.is_object() {
        bail!("{}: expected a JSON object", path.display());
    }
    if let Some(servers) = parsed.get("mcpServers") {
        if !servers.is_object() {
            bail!("{}: mcpServers must be an object", path.display());
        }
    }
    Ok(Some(parsed))
}

/// WP-1103: merges `<project_dir>/.pi/settings.json` over the agent-dir
/// settings, only when the project is trusted (untrusted → the project
/// file is never read — zero effect). The pi-rust settings surface is flat,
/// so the upstream deep merge is a per-field override: each project value
/// that is present wins, absent fields keep the agent-dir value.
///
/// An invalid project file does NOT break the session (upstream: settings
/// load errors are collected and reported, the session continues with the
/// global config): the error names the path and the caller keeps the
/// unmerged config.
pub fn merge_project_settings(
    cfg: AgentConfig,
    project_dir: &Path,
    trusted: bool,
) -> Result<AgentConfig> {
    if !trusted {
        return Ok(cfg);
    }
    let path = project_dir.join(".pi").join("settings.json");
    if !path.exists() {
        return Ok(cfg);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading project settings {}", path.display()))?;
    let parsed: AgentConfig = serde_json::from_str(&raw)
        .with_context(|| format!("parsing project settings {}", path.display()))?;
    // WP-1102 (D4): the closed value set is validated in the project file
    // too (same rule as the agent-dir file).
    if let Some(v) = &parsed.default_project_trust {
        if !matches!(v.as_str(), "ask" | "always" | "never") {
            bail!(
                "invalid defaultProjectTrust value '{v}' in {} (valid: ask, always, never)",
                path.display()
            );
        }
    }
    Ok(AgentConfig {
        default_provider: parsed.default_provider.or(cfg.default_provider),
        default_model: parsed.default_model.or(cfg.default_model),
        default_thinking_level: parsed.default_thinking_level.or(cfg.default_thinking_level),
        session_dir: parsed.session_dir.or(cfg.session_dir),
        default_tools: parsed.default_tools.or(cfg.default_tools),
        theme: parsed.theme.or(cfg.theme),
        default_project_trust: parsed.default_project_trust.or(cfg.default_project_trust),
    })
}

/// Upstream `OVERRIDE_KEYS`: the only keys a project `mcp.json` entry may
/// use to override a global server (it must not touch config or
/// credentials).
const MCP_OVERRIDE_KEYS: [&str; 3] = ["enabled", "exposure", "toolExposure"];

/// WP-1103: merges `<project_dir>/.pi/mcp.json` over the agent-dir MCP
/// config, only when the project is trusted (untrusted → the project file
/// is never read). Upstream semantics:
///
/// - a project entry without `command`/`url`/`type` is an **override**: it
///   may only set `enabled`/`exposure`/`toolExposure` and is merged over
///   the same-named global server (the rest — including credentials — is
///   kept); an override without a global base, or with extra keys, is
///   rejected
/// - a full project entry **replaces** the same-named global server
/// - `auth` on a project `url` server is rejected (global-only — a
///   repository must not pick where the credential goes)
/// - a project name differing only in `-`/`_` from an existing server is
///   rejected (they would share the `mcp__<name>` namespace)
/// - top-level `autoEnableCodemode`: a project boolean overrides the global
///
/// Rejected entries are skipped with a message naming the path; the valid
/// servers (and the agent-dir config when the project file is unusable)
/// are returned — never fatal (upstream: load errors are collected, the
/// session continues).
pub fn merge_project_mcp(
    agent_cfg: Option<Value>,
    project_dir: &Path,
    trusted: bool,
) -> (Option<Value>, Vec<String>) {
    if !trusted {
        return (agent_cfg, Vec::new());
    }
    let path = project_dir.join(".pi").join("mcp.json");
    if !path.exists() {
        return (agent_cfg, Vec::new());
    }
    let mut warnings = Vec::new();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) => {
            warnings.push(format!(
                "{}: reading project mcp config: {e}",
                path.display()
            ));
            return (agent_cfg, warnings);
        }
    };
    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!(
                "{}: parsing project mcp config: {e}",
                path.display()
            ));
            return (agent_cfg, warnings);
        }
    };
    if !parsed.is_object() {
        warnings.push(format!("{}: expected a JSON object", path.display()));
        return (agent_cfg, warnings);
    }
    let mut merged: Value = agent_cfg.unwrap_or_else(|| json!({"mcpServers": {}}));
    if let Some(aec) = parsed.get("autoEnableCodemode") {
        if aec.is_boolean() {
            merged["autoEnableCodemode"] = aec.clone();
        } else {
            warnings.push(format!(
                "{}: autoEnableCodemode must be a boolean",
                path.display()
            ));
        }
    }
    let mut servers = merged
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let project_servers = parsed
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (name, entry) in &project_servers {
        if !entry.is_object() {
            warnings.push(format!(
                "{}: server \"{name}\" must be an object",
                path.display()
            ));
            continue;
        }
        let entry = entry.as_object().unwrap();
        let is_override = entry.get("command").is_none()
            && entry.get("url").is_none()
            && entry.get("type").is_none();
        if is_override {
            let extra: Vec<String> = entry
                .keys()
                .filter(|k| !MCP_OVERRIDE_KEYS.contains(&k.as_str()))
                .cloned()
                .collect();
            match servers.get(name) {
                None => warnings.push(format!(
                    "{}: server \"{name}\" needs \"command\" or \"url\", or a global server to override",
                    path.display()
                )),
                Some(_) if !extra.is_empty() => warnings.push(format!(
                    "{}: server \"{name}\": an override can only set {}, {}, {}",
                    path.display(),
                    MCP_OVERRIDE_KEYS[0],
                    MCP_OVERRIDE_KEYS[1],
                    MCP_OVERRIDE_KEYS[2]
                )),
                Some(base) => {
                    if let Value::Object(base) = base {
                        let mut merged_entry = base.clone();
                        for (k, v) in entry {
                            merged_entry.insert(k.clone(), v.clone());
                        }
                        servers.insert(name.clone(), Value::Object(merged_entry));
                    }
                }
            }
            continue;
        }
        // Full entry: `auth` is global-only for url servers (upstream).
        if entry.get("url").is_some() && entry.get("auth").is_some() {
            warnings.push(format!(
                "{}: server \"{name}\": auth is only allowed in the global mcp.json",
                path.display()
            ));
            continue;
        }
        // Namespace clash: names differing only in `-`/`_` share the
        // `mcp__<name>` tool namespace (upstream `mcpNamespace`).
        let ns = name.replace('-', "_");
        if servers
            .keys()
            .any(|other| other.as_str() != name && other.replace('-', "_") == ns)
        {
            warnings.push(format!(
                "{}: server \"{name}\" conflicts with an existing server namespace",
                path.display()
            ));
            continue;
        }
        servers.insert(name.clone(), Value::Object(entry.clone()));
    }
    merged["mcpServers"] = Value::Object(servers);
    (Some(merged), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-cfg-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_file_gives_defaults() {
        let d = tmpdir();
        let cfg = load(&d).unwrap();
        assert_eq!(cfg, AgentConfig::default());
        let (p, m) = resolve_provider_flags(None, None, &cfg).unwrap();
        assert_eq!((p.as_str(), m), ("fake", None));
    }

    #[test]
    fn loads_core_fields_and_ignores_unknown() {
        let d = tmpdir();
        let mut f = std::fs::File::create(d.join("settings.json")).unwrap();
        write!(
			f,
			r#"{{"defaultProvider":"openai","defaultModel":"gpt-4o","defaultThinkingLevel":"high","sessionDir":"/tmp/s","defaultTools":"read,bash","theme":"dark","tuiMode":"x"}}"#
		)
		.unwrap();
        let cfg = load(&d).unwrap();
        assert_eq!(cfg.default_provider.as_deref(), Some("openai"));
        assert_eq!(cfg.default_model.as_deref(), Some("gpt-4o"));
        assert_eq!(cfg.default_thinking_level.as_deref(), Some("high"));
        assert_eq!(cfg.session_dir.as_deref(), Some("/tmp/s"));
        assert_eq!(cfg.default_tools.as_deref(), Some("read,bash"));
    }

    #[test]
    fn malformed_settings_errors_with_path() {
        let d = tmpdir();
        std::fs::write(d.join("settings.json"), "{not json").unwrap();
        let err = load(&d).unwrap_err().to_string();
        assert!(err.contains("settings.json"), "{err}");
    }

    #[test]
    fn default_project_trust_valid_values_load() {
        for v in ["ask", "always", "never"] {
            let d = tmpdir();
            std::fs::write(
                d.join("settings.json"),
                format!("{{\"defaultProjectTrust\":\"{v}\"}}"),
            )
            .unwrap();
            let cfg = load(&d).unwrap();
            assert_eq!(cfg.default_project_trust.as_deref(), Some(v));
        }
    }

    #[test]
    fn default_project_trust_unknown_value_errors_with_path() {
        let d = tmpdir();
        std::fs::write(
            d.join("settings.json"),
            r#"{"defaultProjectTrust":"sometimes"}"#,
        )
        .unwrap();
        let err = load(&d).unwrap_err().to_string();
        assert!(err.contains("defaultProjectTrust"), "{err}");
        assert!(err.contains("settings.json"), "{err}");
    }

    // --- WP-702/702c: agent-dir mcp.json ---

    #[test]
    fn mcp_config_missing_file_is_none() {
        let d = tmpdir();
        assert_eq!(load_mcp_config(&d).unwrap(), None);
    }

    #[test]
    fn mcp_config_reads_upstream_shape() {
        let d = tmpdir();
        std::fs::write(
            d.join("mcp.json"),
            r#"{"mcpServers":{"fixture":{"command":"node","args":["x.mjs"]}},"other":1}"#,
        )
        .unwrap();
        let cfg = load_mcp_config(&d).unwrap().expect("present");
        assert_eq!(
            cfg["mcpServers"]["fixture"]["command"], "node",
            "upstream shape passes through verbatim (the sidecar validates entries)"
        );
    }

    #[test]
    fn mcp_config_without_mcp_servers_key_is_valid() {
        let d = tmpdir();
        std::fs::write(d.join("mcp.json"), r#"{}"#).unwrap();
        assert_eq!(load_mcp_config(&d).unwrap(), Some(serde_json::json!({})));
    }

    #[test]
    fn mcp_config_malformed_errors_with_path() {
        let d = tmpdir();
        std::fs::write(d.join("mcp.json"), "{not json").unwrap();
        let err = load_mcp_config(&d).unwrap_err().to_string();
        assert!(err.contains("mcp.json"), "{err}");
    }

    #[test]
    fn mcp_config_non_object_shapes_error_with_path() {
        let d = tmpdir();
        std::fs::write(d.join("mcp.json"), r#"[1,2]"#).unwrap();
        assert!(load_mcp_config(&d)
            .unwrap_err()
            .to_string()
            .contains("mcp.json"));
        std::fs::write(d.join("mcp.json"), r#"{"mcpServers":[]}"#).unwrap();
        assert!(load_mcp_config(&d)
            .unwrap_err()
            .to_string()
            .contains("mcpServers"));
    }

    // --- WP-1103: project settings merge (trusted only) ---

    fn proj_dir(_tag: &str) -> (PathBuf, PathBuf) {
        let d = tmpdir();
        let pi = d.join(".pi");
        std::fs::create_dir_all(&pi).unwrap();
        (d, pi)
    }

    #[test]
    fn project_settings_trusted_overrides_fieldwise() {
        let (proj, pi) = proj_dir("a");
        std::fs::write(
            pi.join("settings.json"),
            r#"{"defaultModel":"proj-model","theme":"light"}"#,
        )
        .unwrap();
        let base = AgentConfig {
            default_provider: Some("openai".into()),
            default_model: Some("base-model".into()),
            theme: Some("dark".into()),
            ..Default::default()
        };
        let cfg = merge_project_settings(base, &proj, true).unwrap();
        // Project values win...
        assert_eq!(cfg.default_model.as_deref(), Some("proj-model"));
        assert_eq!(cfg.theme.as_deref(), Some("light"));
        // ...absent project fields keep the agent-dir value.
        assert_eq!(cfg.default_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn project_settings_untrusted_is_never_read() {
        let (proj, pi) = proj_dir("b");
        std::fs::write(pi.join("settings.json"), r#"{"defaultModel":"proj-model"}"#).unwrap();
        let base = AgentConfig {
            default_model: Some("base-model".into()),
            ..Default::default()
        };
        let cfg = merge_project_settings(base.clone(), &proj, false).unwrap();
        assert_eq!(cfg, base);
    }

    #[test]
    fn project_settings_missing_file_keeps_agent_dir() {
        let (proj, _pi) = proj_dir("c");
        let base = AgentConfig {
            default_model: Some("base-model".into()),
            ..Default::default()
        };
        let cfg = merge_project_settings(base.clone(), &proj, true).unwrap();
        assert_eq!(cfg, base);
    }

    #[test]
    fn project_settings_malformed_errors_with_path() {
        let (proj, pi) = proj_dir("d");
        std::fs::write(pi.join("settings.json"), "{not json").unwrap();
        let err = merge_project_settings(AgentConfig::default(), &proj, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("settings.json"), "{err}");
    }

    #[test]
    fn project_settings_bad_default_project_trust_errors() {
        let (proj, pi) = proj_dir("e");
        std::fs::write(
            pi.join("settings.json"),
            r#"{"defaultProjectTrust":"sometimes"}"#,
        )
        .unwrap();
        let err = merge_project_settings(AgentConfig::default(), &proj, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("defaultProjectTrust"), "{err}");
        assert!(err.contains("settings.json"), "{err}");
    }

    // --- WP-1103: project mcp.json merge (trusted only) ---

    fn agent_mcp(servers: &str) -> Value {
        json!({ "mcpServers": serde_json::from_str::<Value>(servers).unwrap() })
    }

    #[test]
    fn mcp_project_full_entry_replaces_global() {
        let (proj, pi) = proj_dir("f");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"srv":{"url":"https://p.example/mcp"}}}"#,
        )
        .unwrap();
        let agent = agent_mcp(r#"{"srv":{"url":"https://g.example/mcp","auth":{"provider":"x"}}}"#);
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        let m = merged.unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(m["mcpServers"]["srv"]["url"], "https://p.example/mcp");
        assert!(
            m["mcpServers"]["srv"].get("auth").is_none(),
            "a full replacement drops the global entry incl. credentials"
        );
    }

    #[test]
    fn mcp_project_override_merges_only_override_keys() {
        let (proj, pi) = proj_dir("g");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"srv":{"enabled":false}}}"#,
        )
        .unwrap();
        let agent = agent_mcp(
            r#"{"srv":{"url":"https://g.example/mcp","headers":{"Authorization":"Bearer s"}}}"#,
        );
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        let m = merged.unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // The override flips enabled; the global rest (incl. the credential)
        // is kept.
        assert_eq!(m["mcpServers"]["srv"]["enabled"], false);
        assert_eq!(m["mcpServers"]["srv"]["url"], "https://g.example/mcp");
        assert_eq!(
            m["mcpServers"]["srv"]["headers"]["Authorization"],
            "Bearer s"
        );
    }

    #[test]
    fn mcp_project_override_without_base_is_rejected() {
        let (proj, pi) = proj_dir("h");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"ghost":{"enabled":false}}}"#,
        )
        .unwrap();
        let (merged, warnings) = merge_project_mcp(Some(agent_mcp("{}")), &proj, true);
        assert_eq!(merged, Some(agent_mcp("{}")));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("ghost"), "{warnings:?}");
        assert!(warnings[0].contains("mcp.json"), "{warnings:?}");
    }

    #[test]
    fn mcp_project_override_with_extra_keys_is_rejected() {
        let (proj, pi) = proj_dir("i");
        // No command/url/type → override; `args` is not an override key.
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"srv":{"enabled":false,"args":["x.mjs"]}}}"#,
        )
        .unwrap();
        let agent = agent_mcp(r#"{"srv":{"url":"https://g.example/mcp"}}"#);
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        // Rejected wholesale: the global entry stays untouched.
        assert_eq!(merged, Some(agent));
        assert!(
            warnings[0].contains("override can only set"),
            "{warnings:?}"
        );
    }

    #[test]
    fn mcp_project_url_server_auth_is_rejected() {
        let (proj, pi) = proj_dir("j");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"srv":{"url":"https://p.example/mcp","auth":{"provider":"x"}}}}"#,
        )
        .unwrap();
        let agent = agent_mcp("{}");
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        assert_eq!(merged, Some(agent));
        assert!(
            warnings[0].contains("auth is only allowed in the global"),
            "{warnings:?}"
        );
    }

    #[test]
    fn mcp_project_namespace_clash_is_rejected() {
        let (proj, pi) = proj_dir("k");
        // `a_b` vs global `a-b`: same `mcp__a_b` namespace.
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"a_b":{"url":"https://p.example/mcp"}}}"#,
        )
        .unwrap();
        let agent = agent_mcp(r#"{"a-b":{"url":"https://g.example/mcp"}}"#);
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        assert_eq!(merged, Some(agent));
        assert!(warnings[0].contains("conflicts"), "{warnings:?}");
    }

    #[test]
    fn mcp_project_adds_new_servers_and_keeps_global_rest() {
        let (proj, pi) = proj_dir("l");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"new":{"command":"node","args":["x.mjs"]}}}"#,
        )
        .unwrap();
        let agent = agent_mcp(r#"{"old":{"url":"https://g.example/mcp"}}"#);
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        assert!(warnings.is_empty(), "{warnings:?}");
        let m = merged.unwrap();
        assert_eq!(m["mcpServers"]["new"]["command"], "node");
        assert_eq!(m["mcpServers"]["old"]["url"], "https://g.example/mcp");
    }

    #[test]
    fn mcp_project_auto_enable_codemode_overrides() {
        let (proj, pi) = proj_dir("m");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"autoEnableCodemode":false,"mcpServers":{}}"#,
        )
        .unwrap();
        let agent = Some(json!({"autoEnableCodemode":true,"mcpServers":{}}));
        let (merged, warnings) = merge_project_mcp(agent.clone(), &proj, true);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(merged.unwrap()["autoEnableCodemode"], false);
    }

    #[test]
    fn mcp_project_malformed_returns_agent_config_with_warning() {
        let (proj, pi) = proj_dir("n");
        std::fs::write(pi.join("mcp.json"), "{not json").unwrap();
        let agent = agent_mcp(r#"{"old":{"url":"https://g.example/mcp"}}"#);
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, true);
        assert_eq!(merged, Some(agent), "the session keeps the global servers");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("mcp.json"), "{warnings:?}");
    }

    #[test]
    fn mcp_project_non_object_shape_is_rejected() {
        let (proj, pi) = proj_dir("o");
        std::fs::write(pi.join("mcp.json"), r#"[1,2]"#).unwrap();
        let (merged, warnings) = merge_project_mcp(None, &proj, true);
        assert_eq!(merged, None);
        assert!(warnings[0].contains("JSON object"), "{warnings:?}");
    }

    #[test]
    fn mcp_project_untrusted_is_never_read() {
        let (proj, pi) = proj_dir("p");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"new":{"url":"https://p.example/mcp"}}}"#,
        )
        .unwrap();
        let agent = agent_mcp("{}");
        let (merged, warnings) = merge_project_mcp(Some(agent.clone()), &proj, false);
        assert_eq!(merged, Some(agent));
        assert!(warnings.is_empty());
    }

    #[test]
    fn mcp_project_without_agent_config_still_applies() {
        let (proj, pi) = proj_dir("q");
        std::fs::write(
            pi.join("mcp.json"),
            r#"{"mcpServers":{"new":{"url":"https://p.example/mcp"}}}"#,
        )
        .unwrap();
        let (merged, warnings) = merge_project_mcp(None, &proj, true);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            merged.unwrap()["mcpServers"]["new"]["url"],
            "https://p.example/mcp"
        );
    }

    #[test]
    fn resolve_flags_matrix() {
        // explicit beats settings
        let cfg = AgentConfig {
            default_provider: Some("openai".into()),
            default_model: Some("gpt-4o".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider_flags(Some("anthropic"), Some("m"), &cfg).unwrap(),
            ("anthropic".into(), Some("m".into()))
        );
        // explicit provider, model from settings
        assert_eq!(
            resolve_provider_flags(Some("anthropic"), None, &cfg).unwrap(),
            ("anthropic".into(), Some("gpt-4o".into()))
        );
        // settings provider/model form
        let cfg2 = AgentConfig {
            default_model: Some("openrouter/xyz".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider_flags(None, None, &cfg2).unwrap(),
            ("openrouter".into(), Some("xyz".into()))
        );
        // bare settings model + settings provider
        assert_eq!(
            resolve_provider_flags(None, None, &cfg).unwrap(),
            ("openai".into(), Some("gpt-4o".into()))
        );
        // bare settings model, no provider anywhere -> error
        let cfg3 = AgentConfig {
            default_model: Some("gpt-4o".into()),
            ..Default::default()
        };
        assert!(resolve_provider_flags(None, None, &cfg3).is_err());
        // --model without --provider -> error
        assert!(resolve_provider_flags(None, Some("m"), &AgentConfig::default()).is_err());
        // nothing anywhere -> fake
        assert_eq!(
            resolve_provider_flags(None, None, &AgentConfig::default()).unwrap(),
            ("fake".into(), None)
        );
    }
}

#[cfg(test)]
mod agent_dir_tests {
    use super::resolve_agent_dir;
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    fn ensure_agent_dir_new_is_owner_only() {
        use super::ensure_agent_dir;
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("pi-rust-agentdir-{}", uuid::Uuid::new_v4()));
        ensure_agent_dir(&d).unwrap();
        let m = std::fs::metadata(&d).unwrap().permissions().mode();
        assert_eq!(
            m & 0o777,
            0o700,
            "fresh agent dir must be 0700, got {:o}",
            m & 0o777
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ensure_agent_dir_existing_keeps_mode() {
        use super::ensure_agent_dir;
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("pi-rust-agentdir2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_agent_dir(&d).unwrap();
        let m = std::fs::metadata(&d).unwrap().permissions().mode();
        assert_eq!(
            m & 0o777,
            0o755,
            "pre-existing dir must keep its mode, got {:o}",
            m & 0o777
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn flag_beats_env_and_home() {
        let p = resolve_agent_dir(
            Some(Path::new("/x/agent")),
            Some("/y/agent"),
            Path::new("/home/u"),
        );
        assert_eq!(p, Path::new("/x/agent"));
    }

    #[test]
    fn env_beats_home_default() {
        let p = resolve_agent_dir(None, Some("/y/agent"), Path::new("/home/u"));
        assert_eq!(
            p,
            Path::new("/y/agent"),
            "PI_CODING_AGENT_DIR wins over ~/.pi/agent"
        );
    }

    #[test]
    fn home_default_is_pi_agent() {
        let p = resolve_agent_dir(None, None, Path::new("/home/u"));
        assert_eq!(p, Path::new("/home/u/.pi/agent"));
    }

    #[test]
    fn empty_env_falls_back_to_home() {
        let p = resolve_agent_dir(None, Some(""), Path::new("/home/u"));
        assert_eq!(p, Path::new("/home/u/.pi/agent"));
    }
}

/// Upstream `getAgentDir` resolution (without the flag, which the caller
/// passes): env `PI_CODING_AGENT_DIR` > `~/.pi/agent`.
/// WP-906: create the agent dir; owner-only (0700) when pi-rust created it.
/// The dir holds auth.json (API keys) and session transcripts, which must not
/// be world-readable on shared hosts. Pre-existing dirs keep their mode.
/// Advisory R2-002: the `create_dir` result is the authoritative "we
/// created it" signal — no `exists()` pre-check.
pub fn ensure_agent_dir(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating agent dir parent {}", parent.display()))?;
    }
    match std::fs::create_dir(path) {
        Ok(()) => {
            // Only dirs pi-rust just created get 0700.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("setting 0700 on agent dir {}", path.display()))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(e).with_context(|| format!("creating agent dir {}", path.display()));
        }
    }
    Ok(())
}

pub fn resolve_agent_dir(
    flag: Option<&std::path::Path>,
    env_dir: Option<&str>,
    home: &std::path::Path,
) -> std::path::PathBuf {
    if let Some(f) = flag {
        return f.to_path_buf();
    }
    if let Some(e) = env_dir {
        if !e.is_empty() {
            return std::path::PathBuf::from(e);
        }
    }
    home.join(".pi").join("agent")
}
