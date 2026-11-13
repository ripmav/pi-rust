//! Agent config from `<agent-dir>/settings.json` (WP-301).
//!
//! Upstream reads the global settings file from the agent dir; project
//! `cwd/.pi/settings.json` is only merged in for trusted projects — the
//! headless core has no trust model, so it reads the global file only
//! (WP-301 scope; recorded in the WP file).
//!
//! Unknown keys are ignored (forward compat). Missing file = all defaults.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
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
/// rule as settings.json). Project `<cwd>/.pi/mcp.json` is a documented v0
/// non-goal (agent-dir only, same rule as settings.json).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
