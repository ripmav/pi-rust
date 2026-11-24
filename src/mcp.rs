//! Core-native MCP hosting (WP-1403/E3S3): the core connects and drives
//! the MCP servers of the merged mcp config (agent-dir `mcp.json` +
//! trusted project `.pi/mcp.json`, `config::merge_project_mcp`) itself —
//! JSON-RPC 2.0 over stdio (newline-delimited framing, the MCP spec —
//! upstream pi-mcp `transports/stdio.ts` + `lines.ts`) and streamable
//! HTTP (SSE response assembly — `transports/streamable-http.ts`).
//! Tool calls execute natively: no `extension_tool_call` hop.
//!
//! Upstream ports (pin 64a15f4):
//! - naming: `packages/mcp/src/naming.ts` — `mcp__<server>__<tool>`,
//!   sanitized to `[A-Za-z0-9_]`, 64-char cap with an 8-char sha256
//!   suffix on collision/overflow.
//! - schemas: `extensions/mcp/tools.ts` — the server's `inputSchema`
//!   normalized to an object schema; description fallback chain.
//! - exposure: `core/mcp-servers.ts` — default `codemode`; `toolExposure`
//!   exact match > first `*` pattern > server default; the alias
//!   `codemode-deferred` resolves to `codemode`. Only `direct` tools are
//!   declared to the model (the sidecar path's initial-activation rule).
//! - lifecycle: `extensions/mcp/runtime.ts` — background connect on
//!   session start; NO auto-restart (a crashed server reconnects lazily
//!   on the next call, fresh process); close on session shutdown.
//!
//! Ownership (D13, final form E3S5): the core is the ONLY connector of
//! MCP servers — the sidecar hosts no MCP servers any more (its built-in
//! mcp extension + the `mcpConfig` registration path were removed in
//! E3S5), so nothing is double-connected. Servers carrying `oauth`/`auth`
//! fields are skipped with a warning (the core cannot authenticate them
//! yet — D17, and no host remains). `pi.registerMcpServer` registrations
//! by user extensions are FORWARDED to the core over the bridge
//! (`mcp_server_register`, additive in protocol v6 — E3S5 follow-up) and
//! hosted exactly like configured servers; a rejected forward (invalid
//! entry, duplicate, `oauth`/`auth` — D17) is a typed error the sidecar
//! surfaces to the extension.
//!
//! v0 core limits (the sidecar serves none of these any more — E3S5):
//! OAuth/`auth` server fields (typed skip), resource tools,
//! the `/mcp` manager, the `mcp_servers` prompt section, and
//! codemode/deferred reachability (no core codemode/tool_search — such
//! tools are registered but only reachable by explicit name).

use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// MCP message size limit (upstream `DEFAULT_MAX_MESSAGE_BYTES`).
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Stderr tail kept for connect-failure diagnostics (upstream
/// `STDERR_TAIL_CHARS`).
const STDERR_TAIL_CHARS: usize = 2_000;
/// The first prompt waits this long for `direct` servers (upstream
/// `DEFAULT_STARTUP_WAIT_MS`).
pub const STARTUP_WAIT_MS: u64 = 10_000;
const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const EXPOSURES: [&str; 4] = ["codemode", "deferred", "direct", "hidden"];
const MAX_TOOL_NAME_LENGTH: usize = 64;

// ---------------------------------------------------------------------------
// config parsing & validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Kind {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub kind: Kind,
    /// `enabled: false` servers never connect (upstream: no connection).
    pub enabled: bool,
    /// Resolved exposure (alias applied); default `codemode`.
    pub exposure: String,
    /// Tool-name (or `*` pattern) -> exposure overrides.
    pub tool_exposure: BTreeMap<String, String>,
    pub timeout_secs: u64,
    /// The original JSON entry (mcp.json / the forwarded
    /// `registerMcpServer` config) — the `mcp_servers_change` payload
    /// shape (upstream `RegisteredMcpServer.config`). Null for test
    /// configs that never cross the wire.
    pub raw: Value,
    /// Set for servers an extension registered (`pi.registerMcpServer`
    /// forwarded over the bridge) — the upstream
    /// `RegisteredMcpServer.extensionPath` field.
    pub extension_path: Option<String>,
}

/// `*`-pattern match (upstream `toolPatternRegExp`): `*` matches any
/// characters, everything else is a literal.
pub fn pattern_match(pattern: &str, name: &str) -> bool {
    let mut glob: Vec<char> = Vec::new();
    for part in pattern.split('*') {
        for c in part.chars() {
            if matches!(
                c,
                '.' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
            ) {
                glob.push('\\');
            }
            glob.push(c);
        }
        glob.push('*');
    }
    glob_match(&glob, name)
}

/// Matches a glob anchored at both ends, where `*` = any run of chars
/// and `\c` = literal `c` (all `pattern_match` can produce).
fn glob_match(glob: &[char], name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let mut i = 0usize;
    let mut j = 0usize;
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while j < name.len() {
        if i < glob.len()
            && (glob[i] == name[j]
                || (glob[i] == '\\' && i + 1 < glob.len() && glob[i + 1] == name[j]))
        {
            i += if glob[i] == '\\' { 2 } else { 1 };
            j += 1;
        } else if i < glob.len() && glob[i] == '*' {
            if star.is_none() {
                star = Some(i);
                mark = j;
            }
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < glob.len() && glob[i] == '*' {
        i += 1;
    }
    i == glob.len()
}

/// Exposure of one tool (upstream `getMcpToolExposure`): exact
/// `toolExposure` name, else the first `*` pattern match, else the
/// server default. BTreeMap key order stands in for the upstream
/// object-insertion order (deterministic; documented v0 difference).
pub fn tool_exposure(cfg: &ServerConfig, tool: &str) -> String {
    if let Some(e) = cfg.tool_exposure.get(tool) {
        return e.clone();
    }
    for (p, e) in &cfg.tool_exposure {
        if p.contains('*') && pattern_match(p, tool) {
            return e.clone();
        }
    }
    cfg.exposure.clone()
}

/// The exposures a server's tools can have from its config alone
/// (upstream `configuredExposures`).
pub fn configured_exposures(cfg: &ServerConfig) -> Vec<String> {
    let mut set = vec![cfg.exposure.clone()];
    for e in cfg.tool_exposure.values() {
        if !set.contains(e) {
            set.push(e.clone());
        }
    }
    set
}

/// Whether some of the server's tools are declared to the model, so the
/// first prompt waits for it (upstream `hasDirectTools`).
pub fn has_direct_tools(cfg: &ServerConfig) -> bool {
    configured_exposures(cfg).contains(&"direct".to_string())
}

/// Upstream `resolve-config-value` template resolution (minus command
/// substitution): `${NAME}` / `$NAME` env refs, `$$` and `$!` escapes,
/// bare `$` literal. `!`-prefixed values are rejected (D12 rule: no
/// shell execution from a config file). Unset var = error.
/// Resolve env references in a config value (upstream
/// `resolve-config-value` template subset; D12 house rule: `!command`
/// substitution is a typed rejection — the core does not run shells from
/// config files). `${NAME}` and `$NAME` expand from the process env; an
/// unset (or empty) variable is an error naming it; `$$` → `$` and `$!`
/// → `!`; anything else after `$` is a literal `$`.
pub fn resolve_value(ctx: &str, value: &str) -> Result<String, String> {
    if value.starts_with('!') {
        return Err(format!(
            "{ctx}: `!command` substitution is not supported by the core (no shell execution from a config file; D12)"
        ));
    }
    let env_err = |name: &str| format!("{ctx}: env var {name} is not set");
    let lookup = |name: &str| -> Result<String, String> {
        std::env::var(name)
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| env_err(name))
    };
    let mut out = String::new();
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if i + 1 >= chars.len() {
            out.push('$');
            i += 1;
            continue;
        }
        match chars[i + 1] {
            '$' => {
                out.push('$');
                i += 2;
            }
            '!' => {
                out.push('!');
                i += 2;
            }
            '{' => {
                if let Some(end) = chars[i + 2..].iter().position(|&c| c == '}') {
                    let name: String = chars[i + 2..i + 2 + end].iter().collect();
                    if is_env_name(&name) {
                        out.push_str(&lookup(&name)?);
                    } else {
                        // Upstream: an invalid reference is a literal.
                        out.push_str(&value[i..i + 2 + end + 1]);
                    }
                    i += end + 3;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            _ if is_env_name_start(chars[i + 1]) => {
                let mut end = i + 1;
                while end < chars.len() && is_env_name_cont(chars[end]) {
                    end += 1;
                }
                let name: String = chars[i + 1..end].iter().collect();
                out.push_str(&lookup(&name)?);
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

fn is_env_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_env_name_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn is_env_name(name: &str) -> bool {
    let mut it = name.chars();
    matches!(it.next(), Some(c) if is_env_name_start(c)) && it.all(is_env_name_cont)
}

/// Upstream `expandHome`: `~` and `~/…` name the home directory.
fn expand_home(value: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if value == "~" {
        home
    } else if let Some(rest) = value.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        value.to_string()
    }
}

fn resolve_exposure(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    let s = if s == "codemode-deferred" {
        "codemode"
    } else {
        s
    };
    EXPOSURES.contains(&s).then(|| s.to_string())
}

fn exposure_list() -> String {
    EXPOSURES
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One stdio server entry (upstream `McpStdioServerConfig` fields).
fn parse_stdio(ctx: &str, obj: &serde_json::Map<String, Value>) -> Result<Kind, String> {
    let command = match obj.get("command").and_then(Value::as_str) {
        Some(c) if !c.trim().is_empty() => expand_home(c),
        Some(_) => return Err(format!("{ctx}: \"command\" must not be empty")),
        None => unreachable!("caller checked"),
    };
    let args = match obj.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let Some(a) = v.as_array() else {
                return Err(format!("{ctx}: \"args\" must be an array of strings"));
            };
            let mut out = Vec::new();
            for (n, item) in a.iter().enumerate() {
                let Some(s) = item.as_str() else {
                    return Err(format!("{ctx}: args[{n}] must be a string"));
                };
                out.push(expand_home(s));
            }
            out
        }
    };
    let mut env = BTreeMap::new();
    if let Some(e) = obj.get("env") {
        let Some(m) = e.as_object() else {
            return Err(format!("{ctx}: \"env\" must be an object of strings"));
        };
        for (k, v) in m {
            let Some(s) = v.as_str() else {
                return Err(format!("{ctx}: env \"{k}\" must be a string"));
            };
            env.insert(k.clone(), resolve_value(&format!("{ctx} env \"{k}\""), s)?);
        }
    }
    let cwd = obj.get("cwd").and_then(Value::as_str).map(expand_home);
    Ok(Kind::Stdio {
        command,
        args,
        env,
        cwd,
    })
}

/// One http server entry (upstream `McpHttpServerConfig` fields, minus
/// the oauth/auth the core does not support yet — those are skipped by
/// the caller).
fn parse_http(ctx: &str, obj: &serde_json::Map<String, Value>) -> Result<Kind, String> {
    let url = match obj.get("url").and_then(Value::as_str) {
        Some(u) => u.to_string(),
        None => unreachable!("caller checked"),
    };
    match url.parse::<reqwest::Url>() {
        Ok(u) if matches!(u.scheme(), "http" | "https") => {}
        Ok(_) => return Err(format!("{ctx}: \"url\" must be http(s)")),
        Err(e) => return Err(format!("{ctx}: invalid \"url\": {e}")),
    }
    let mut headers = BTreeMap::new();
    if let Some(h) = obj.get("headers") {
        let Some(m) = h.as_object() else {
            return Err(format!("{ctx}: \"headers\" must be an object of strings"));
        };
        for (k, v) in m {
            let Some(s) = v.as_str() else {
                return Err(format!("{ctx}: header \"{k}\" must be a string"));
            };
            headers.insert(
                k.clone(),
                resolve_value(&format!("{ctx} header \"{k}\""), s)?,
            );
        }
    }
    Ok(Kind::Http { url, headers })
}

/// Parses + validates the merged mcp config (the `mcpServers` value).
/// Returns (servers, warnings): an invalid entry is skipped with a
/// message naming it, never fatal (upstream: load errors are collected,
/// the session continues).
pub fn parse_servers(cfg: Option<&Value>) -> (Vec<ServerConfig>, Vec<String>) {
    let mut servers = Vec::new();
    let mut warnings = Vec::new();
    let Some(map) = cfg
        .and_then(|c| c.get("mcpServers"))
        .and_then(Value::as_object)
    else {
        return (servers, warnings);
    };
    for (name, entry) in map {
        let ctx = format!("mcp server \"{name}\"");
        let Some(obj) = entry.as_object() else {
            warnings.push(format!("{ctx}: entry must be an object"));
            continue;
        };
        if obj.get("oauth").is_some() || obj.get("auth").is_some() {
            warnings.push(format!(
                "{ctx}: oauth/auth is not supported by the core yet (D17) — server skipped (the sidecar no longer hosts MCP servers, E3S5)"
            ));
            continue;
        }
        let (has_command, has_url) = (obj.contains_key("command"), obj.contains_key("url"));
        let declared_type = obj.get("type").and_then(Value::as_str);
        let kind = match (has_command, has_url, declared_type) {
            (true, true, _) => {
                warnings.push(format!(
                    "{ctx}: exactly one of \"command\" or \"url\" must be set"
                ));
                continue;
            }
            (true, false, Some("stdio") | None) => match parse_stdio(&ctx, obj) {
                Ok(k) => k,
                Err(e) => {
                    warnings.push(e);
                    continue;
                }
            },
            (false, true, Some("http") | None) => match parse_http(&ctx, obj) {
                Ok(k) => k,
                Err(e) => {
                    warnings.push(e);
                    continue;
                }
            },
            (_, _, Some(t)) => {
                warnings.push(format!("{ctx}: unknown type \"{t}\" (valid: stdio, http)"));
                continue;
            }
            _ => {
                warnings.push(format!(
                    "{ctx}: needs \"command\" (stdio) or \"url\" (http)"
                ));
                continue;
            }
        };
        let exposure = match obj.get("exposure") {
            None => "codemode".to_string(),
            Some(v) => match resolve_exposure(v) {
                Some(e) => e,
                None => {
                    warnings.push(format!(
                        "{ctx}: exposure must be one of {}",
                        exposure_list()
                    ));
                    continue;
                }
            },
        };
        let mut tool_exposure = BTreeMap::new();
        if let Some(te) = obj.get("toolExposure") {
            let Some(m) = te.as_object() else {
                warnings.push(format!(
                    "{ctx}: \"toolExposure\" must map tool names to exposures"
                ));
                continue;
            };
            let mut bad = false;
            for (k, v) in m {
                match resolve_exposure(v) {
                    Some(e) => {
                        tool_exposure.insert(k.clone(), e);
                    }
                    None => {
                        warnings.push(format!(
                            "{ctx}: toolExposure \"{k}\" must be one of {}",
                            exposure_list()
                        ));
                        bad = true;
                        break;
                    }
                }
            }
            if bad || tool_exposure.len() != m.len() {
                continue;
            }
        }
        let timeout_secs = match obj.get("timeout") {
            None => DEFAULT_TIMEOUT_SECS,
            Some(v) => {
                let Some(n) = v.as_u64().or_else(|| {
                    v.as_f64()
                        .and_then(|f| (f >= 1.0).then_some(f.round() as u64))
                }) else {
                    warnings.push(format!("{ctx}: \"timeout\" must be a number of seconds"));
                    continue;
                };
                n
            }
        };
        let enabled = obj.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        servers.push(ServerConfig {
            name: name.clone(),
            kind,
            enabled,
            exposure,
            tool_exposure,
            timeout_secs,
            raw: entry.clone(),
            extension_path: None,
        });
    }
    (servers, warnings)
}

// ---------------------------------------------------------------------------
// naming (upstream `naming.ts`)
// ---------------------------------------------------------------------------

/// `mcp__<server>`, `-` replaced by `_` (upstream `mcpNamespace`).
pub fn mcp_namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

fn sanitize(c: char) -> char {
    if c.is_ascii_alphanumeric() || c == '_' {
        c
    } else {
        '_'
    }
}

/// `mcp__<server>__<tool>` sanitized; too long or `is_taken` -> hash
/// suffix (upstream `createMcpToolName`).
pub fn mcp_tool_name(server: &str, tool: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let raw: String = format!("mcp__{server}__{tool}")
        .chars()
        .map(sanitize)
        .collect();
    if raw.len() <= MAX_TOOL_NAME_LENGTH && !is_taken(&raw) {
        return raw;
    }
    let hash = {
        let mut h = sha2::Sha256::new();
        h.update(format!("{server}\0{tool}"));
        let digest = h.finalize();
        let mut s = String::new();
        for b in digest.iter().take(4) {
            s.push_str(&format!("{b:02x}"));
        }
        s
    };
    let cut = MAX_TOOL_NAME_LENGTH - hash.len() - 1;
    let base: String = raw.chars().take(cut).collect();
    format!("{base}_{hash}")
}

// ---------------------------------------------------------------------------
// tool table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ServerTool {
    /// The server's own tool name (what `tools/call` sends).
    pub original: String,
    pub description: Option<String>,
    pub title: Option<String>,
    pub input_schema: Value,
}

/// A registered tool (any exposure, connected servers only).
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: String,
    /// `<server>/<original tool>` label for logs and the future `/mcp` manager.
    pub label: String,
    pub description: String,
    pub parameters: Value,
    pub exposure: String,
}

impl ToolEntry {
    /// The `ready.tools` entry shape the core's merged tool set consumes.
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "label": self.label,
            "description": self.description,
            "parameters": self.parameters,
            "exposure": self.exposure,
        })
    }
}

/// Upstream `normalizeMcpInputSchema`: object type + properties present.
fn normalize_input_schema(schema: &Value) -> Value {
    let Some(obj) = schema.as_object() else {
        return json!({"type": "object", "properties": {}});
    };
    let mut out = obj.clone();
    if !out.contains_key("type") {
        out.insert("type".into(), json!("object"));
    }
    if !out.contains_key("properties") {
        out.insert("properties".into(), json!({}));
    }
    Value::Object(out)
}

// ---------------------------------------------------------------------------
// the host
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Enabled; connect in flight (or never attempted).
    Starting,
    Connected,
    /// Was connected; the transport dropped (upstream: reconnects on the
    /// next call; no auto-restart).
    Disconnected,
    /// The last connect attempt failed (a call retries once).
    Failed,
    /// `enabled: false` — no connection.
    Disabled,
    /// The host closed (session shutdown).
    Closed,
}

enum ActorCmd {
    Request {
        id: u64,
        params: Value,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Gen(u64);

enum ServerUpdate {
    Connected {
        gen: Gen,
        name: String,
        tools: Vec<ServerTool>,
    },
    Disconnected {
        gen: Gen,
        name: String,
        error: String,
    },
    Failed {
        gen: Gen,
        name: String,
        error: String,
    },
    /// A `tools/list_changed` refresh landed (connected server only).
    Tools {
        gen: Gen,
        name: String,
        tools: Vec<ServerTool>,
    },
}

impl ServerUpdate {
    fn name(&self) -> &str {
        match self {
            ServerUpdate::Connected { name, .. }
            | ServerUpdate::Disconnected { name, .. }
            | ServerUpdate::Failed { name, .. }
            | ServerUpdate::Tools { name, .. } => name,
        }
    }
    fn gen(&self) -> Gen {
        match self {
            ServerUpdate::Connected { gen, .. }
            | ServerUpdate::Disconnected { gen, .. }
            | ServerUpdate::Failed { gen, .. }
            | ServerUpdate::Tools { gen, .. } => *gen,
        }
    }
}

struct Server {
    cfg: ServerConfig,
    state: ConnState,
    error: Option<String>,
    tools: Vec<ServerTool>,
    gen: Gen,
    actor: Option<mpsc::UnboundedSender<ActorCmd>>,
    done: Option<oneshot::Receiver<()>>,
}

/// The core's MCP host: one entry per configured server. The tool table
/// is rebuilt by the pump on every connection update; the agent loop
/// reads it per turn.
pub struct McpHost {
    inner: Arc<Inner>,
}

struct Inner {
    servers: Mutex<BTreeMap<String, Server>>,
    /// All registered tools (any exposure, connected servers only).
    tools: Mutex<Vec<ToolEntry>>,
    /// Sanitized tool name -> (server, original tool); hidden tools are
    /// excluded (unreachable, upstream).
    by_name: Mutex<HashMap<String, (String, String)>>,
    /// `direct`-exposure tools in the `ready.tools` entry shape — what
    /// the model is declared.
    declared: Mutex<Vec<Value>>,
    update_tx: mpsc::UnboundedSender<ServerUpdate>,
    update_rx: Mutex<Option<mpsc::UnboundedReceiver<ServerUpdate>>>,
    cwd: PathBuf,
    gen_seq: AtomicU64,
    closed: AtomicBool,
    /// First-prompt wait already happened (upstream `waitedForStartup`,
    /// once per session = once per process here).
    waited: AtomicBool,
}

impl McpHost {
    /// Builds the host from the merged mcp config (absent = no servers).
    /// Returns the warnings for invalid entries (the caller logs them).
    pub fn new(cfg: Option<&Value>, cwd: &Path) -> (Self, Vec<String>) {
        let (servers, warnings) = parse_servers(cfg);
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let mut map = BTreeMap::new();
        for scfg in servers {
            let state = if scfg.enabled {
                ConnState::Starting
            } else {
                ConnState::Disabled
            };
            map.insert(
                scfg.name.clone(),
                Server {
                    cfg: scfg,
                    state,
                    error: None,
                    tools: Vec::new(),
                    gen: Gen(0),
                    actor: None,
                    done: None,
                },
            );
        }
        (
            Self {
                inner: Arc::new(Inner {
                    servers: Mutex::new(map),
                    tools: Mutex::new(Vec::new()),
                    by_name: Mutex::new(HashMap::new()),
                    declared: Mutex::new(Vec::new()),
                    update_tx,
                    update_rx: Mutex::new(Some(update_rx)),
                    cwd: cwd.to_path_buf(),
                    gen_seq: AtomicU64::new(0),
                    closed: AtomicBool::new(false),
                    waited: AtomicBool::new(false),
                }),
            },
            warnings,
        )
    }

    /// The update pump + the session-start connects (upstream: the mcp
    /// extension connects on `session_start`, in the background).
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut rx = this.inner.update_rx.lock().unwrap().take().unwrap();
            while let Some(upd) = rx.recv().await {
                this.apply_update(upd);
            }
        });
        let this = Arc::clone(self);
        for name in self.configured_names() {
            if this.server_enabled(&name) {
                let _ = this.start_attempt(&name);
            }
        }
    }

    pub fn configured_names(&self) -> Vec<String> {
        self.inner.servers.lock().unwrap().keys().cloned().collect()
    }

    fn server_enabled(&self, name: &str) -> bool {
        self.inner
            .servers
            .lock()
            .unwrap()
            .get(name)
            .is_some_and(|s| s.state != ConnState::Disabled)
    }

    /// A server's config (None = the name is not a configured server).
    pub fn server_config(&self, name: &str) -> Option<ServerConfig> {
        self.inner
            .servers
            .lock()
            .unwrap()
            .get(name)
            .map(|s| s.cfg.clone())
    }

    /// The configured server whose namespace (`mcp__<server>__`)
    /// prefixes the tool name (longest server name wins); the tool part
    /// is the (sanitized) remainder.
    pub fn server_prefix_of(&self, tool_name: &str) -> Option<(String, String)> {
        let mut best: Option<(String, usize)> = None;
        let g = self.inner.servers.lock().unwrap();
        for name in g.keys() {
            let prefix = format!("{}__", mcp_namespace(name));
            if tool_name.starts_with(&prefix)
                && best.as_ref().is_none_or(|(_, len)| prefix.len() > *len)
            {
                best = Some((name.clone(), prefix.len()));
            }
        }
        best.map(|(name, len)| (name, tool_name[len..].to_string()))
    }

    /// The (server, original tool) a registered tool name belongs to
    /// (hidden tools excluded).
    pub fn owner_of(&self, tool_name: &str) -> Option<(String, String)> {
        self.inner.by_name.lock().unwrap().get(tool_name).cloned()
    }

    /// The `direct`-exposure tools, in the `ready.tools` entry shape —
    /// what the model is declared (E3S3: core-native, sidecar optional).
    pub fn declared_tools(&self) -> Vec<Value> {
        self.inner.declared.lock().unwrap().clone()
    }

    fn apply_update(&self, upd: ServerUpdate) {
        if self.inner.closed.load(Ordering::SeqCst) {
            return;
        }
        let name = upd.name().to_string();
        {
            let mut g = self.inner.servers.lock().unwrap();
            let Some(s) = g.get_mut(&name) else {
                return;
            };
            // A stale attempt's late update (a replaced connection) must
            // not stomp the current state.
            if upd.gen() != s.gen {
                return;
            }
            match upd {
                ServerUpdate::Connected { tools, .. } => {
                    s.state = ConnState::Connected;
                    s.error = None;
                    s.tools = tools;
                }
                ServerUpdate::Disconnected { error, .. } => {
                    s.state = ConnState::Disconnected;
                    s.error = Some(error);
                    s.actor = None;
                    s.done = None;
                }
                ServerUpdate::Failed { error, .. } => {
                    s.state = ConnState::Failed;
                    s.error = Some(error);
                    s.actor = None;
                    s.done = None;
                }
                ServerUpdate::Tools { tools, .. } => {
                    if s.state == ConnState::Connected {
                        s.tools = tools;
                    }
                }
            }
        }
        self.rebuild_tools();
        crate::corelog::line(&format!("[core] mcp: {}", self.state_line(&name)));
    }

    fn state_line(&self, name: &str) -> String {
        let g = self.inner.servers.lock().unwrap();
        match g.get(name) {
            None => format!("\"{name}\": unknown"),
            Some(s) => match s.state {
                ConnState::Connected => {
                    format!("\"{}\": connected ({} tools)", name, s.tools.len())
                }
                ConnState::Disconnected => format!(
                    "\"{}\": disconnected ({})",
                    name,
                    s.error
                        .clone()
                        .unwrap_or_else(|| "connection closed".into())
                ),
                ConnState::Failed => format!(
                    "\"{}\": failed to connect: {}",
                    name,
                    s.error.clone().unwrap_or_else(|| "unknown error".into())
                ),
                ConnState::Disabled => format!("\"{name}\": disabled"),
                ConnState::Closed => format!("\"{name}\": closed"),
                ConnState::Starting => format!("\"{name}\": connecting"),
            },
        }
    }

    /// Rebuilds the tool table from the connected servers. Name
    /// assignment follows upstream `registerTools`: all tools whose
    /// sanitized names collide get the hash suffix, so the plain-name
    /// winner does not depend on list order.
    fn rebuild_tools(&self) {
        let servers = self.inner.servers.lock().unwrap();
        let mut by_name: HashMap<String, (String, String)> = HashMap::new();
        let mut tools: Vec<ToolEntry> = Vec::new();
        let mut declared: Vec<Value> = Vec::new();
        for (server, srv) in servers.iter() {
            if srv.state != ConnState::Connected {
                continue;
            }
            let plain: Vec<String> = srv
                .tools
                .iter()
                .map(|t| mcp_tool_name(server, &t.original, |_| false))
                .collect();
            let mut current: Vec<String> = Vec::new();
            for t in &srv.tools {
                let name = mcp_tool_name(server, &t.original, |c| {
                    by_name.get(c).is_some_and(|(s, _)| s != server)
                        || current.iter().any(|s| s == c)
                        || (plain.iter().position(|p| p == c).is_some()
                            && plain.iter().rposition(|p| p == c)
                                != plain.iter().position(|p| p == c))
                });
                current.push(name.clone());
                let exposure = tool_exposure(&srv.cfg, &t.original);
                by_name.insert(name.clone(), (server.clone(), t.original.clone()));
                if exposure == "hidden" {
                    continue; // unreachable (upstream)
                }
                let description = t
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .or(t.title.as_deref())
                    .unwrap_or(&format!("MCP tool {} from server {server}", t.original))
                    .to_string();
                let entry = ToolEntry {
                    name: name.clone(),
                    label: format!("{server}/{}", t.original),
                    description,
                    parameters: normalize_input_schema(&t.input_schema),
                    exposure: exposure.clone(),
                };
                if exposure == "direct" {
                    declared.push(entry.to_json());
                }
                tools.push(entry);
            }
        }
        *self.inner.tools.lock().unwrap() = tools;
        *self.inner.by_name.lock().unwrap() = by_name;
        *self.inner.declared.lock().unwrap() = declared;
    }

    /// Starts one connect attempt for the server (idempotent per
    /// attempt; a live attempt is returned as-is).
    fn start_attempt(&self, name: &str) -> Option<()> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return None;
        }
        let (cfg, cwd, cmd_rx, done_tx, gen) = {
            let mut g = self.inner.servers.lock().unwrap();
            let s = g.get_mut(name)?;
            if s.state == ConnState::Connected || s.actor.is_some() {
                return Some(());
            }
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
            let (done_tx, done_rx) = oneshot::channel();
            let gen = Gen(self.inner.gen_seq.fetch_add(1, Ordering::SeqCst) + 1);
            s.gen = gen;
            s.state = ConnState::Starting;
            s.actor = Some(cmd_tx);
            s.done = Some(done_rx);
            (s.cfg.clone(), self.inner.cwd.clone(), cmd_rx, done_tx, gen)
        };
        let update_tx = self.inner.update_tx.clone();
        let kind = cfg.kind.clone();
        let timeout = Duration::from_secs(cfg.timeout_secs);
        let server_name = name.to_string();
        tokio::spawn(async move {
            match &kind {
                Kind::Stdio { .. } => {
                    stdio_actor(&server_name, &cfg, &cwd, gen, cmd_rx, update_tx, timeout).await;
                }
                Kind::Http { .. } => {
                    http_actor(&server_name, &cfg, gen, cmd_rx, update_tx, timeout).await;
                }
            }
            let _ = done_tx.send(());
        });
        Some(())
    }

    /// Connects the server if needed (lazy reconnect per upstream: a
    /// crashed/failed server reconnects on the next call, fresh
    /// transport; no auto-restart). Up to one attempt per poll pass; a
    /// server that keeps failing surfaces its last connect error.
    pub async fn ensure_connected(&self, name: &str) -> Result<(), String> {
        let deadline = Instant::now()
            + Duration::from_secs(
                self.server_config(name)
                    .map(|c| c.timeout_secs.saturating_mul(2) + 10)
                    .unwrap_or(130),
            );
        let mut started = 0;
        loop {
            let action = {
                let g = self.inner.servers.lock().unwrap();
                match g.get(name) {
                    None => return Err(format!("unknown MCP server \"{name}\"")),
                    Some(s) if s.state == ConnState::Connected => return Ok(()),
                    Some(s) if s.state == ConnState::Disabled => {
                        return Err(format!("MCP server \"{name}\" is disabled."))
                    }
                    Some(s) if s.state == ConnState::Closed => {
                        return Err(format!("MCP server \"{name}\" is shut down."))
                    }
                    Some(s) if s.actor.is_some() => None, // an attempt is in flight: wait
                    Some(_) => Some(()),                  // start one
                }
            };
            if action.is_some() {
                started += 1;
                if started > 3 {
                    return Err(self.connect_error(name));
                }
                self.start_attempt(name);
            }
            if Instant::now() >= deadline {
                return Err(self.connect_error(name));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The typed connect error for a server that is not connected (the last
    /// attempt's error where there is one).
    fn connect_error(&self, name: &str) -> String {
        let g = self.inner.servers.lock().unwrap();
        match g.get(name) {
            Some(s) if s.state == ConnState::Failed => format!(
                "MCP server \"{name}\" failed to connect: {}",
                s.error.clone().unwrap_or_else(|| "unknown error".into())
            ),
            Some(s) if s.state == ConnState::Starting => {
                format!("MCP server \"{name}\" is still connecting (timed out)")
            }
            Some(s) if s.state == ConnState::Disconnected => format!(
                "MCP server \"{name}\" is disconnected: {}",
                s.error
                    .clone()
                    .unwrap_or_else(|| "connection closed".into())
            ),
            _ => format!("MCP server \"{name}\" is not connected (see the mcp log lines)"),
        }
    }

    /// The first prompt waits for servers whose configured exposure
    /// includes `direct` (upstream `waitForDirectServers`, 10 s cap,
    /// once per session). Settled = connected or failed.
    pub async fn await_direct(&self) {
        if self.inner.waited.swap(true, Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(STARTUP_WAIT_MS);
        loop {
            let unsettled = {
                let g = self.inner.servers.lock().unwrap();
                g.values().any(|s| {
                    s.cfg.enabled && s.state == ConnState::Starting && has_direct_tools(&s.cfg)
                })
            };
            if !unsettled {
                return;
            }
            if Instant::now() >= deadline {
                crate::corelog::line(
                    "[core] mcp: servers are still connecting; their tools become available once connected",
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Calls `tools/call` on a configured server's tool. `tool` is the
    /// name the model sees (sanitized, possibly hashed); the server is
    /// sent its original tool name (upstream `toolOwners` inverse). Error
    /// strings are the tool-result text (upstream-shaped).
    pub async fn call(
        &self,
        server: &str,
        tool: &str,
        args: &Value,
    ) -> Result<(Value, bool), String> {
        let cfg = self
            .server_config(server)
            .ok_or_else(|| format!("unknown MCP server \"{server}\""))?;
        let original = self
            .inner
            .by_name
            .lock()
            .unwrap()
            .get(tool)
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| tool.to_string());
        if tool_exposure(&cfg, &original) == "hidden" {
            return Err(format!(
                "MCP tool \"{server}/{original}\" is no longer available."
            ));
        }
        self.ensure_connected(server).await?;
        // The tool must be offered by the server as connected (upstream
        // "no longer available").
        let (actor, original) = {
            let g = self.inner.servers.lock().unwrap();
            let s = g.get(server).unwrap();
            let Some(t) = s.tools.iter().find(|t| t.original == original) else {
                return Err(format!(
                    "MCP tool \"{server}/{original}\" is no longer available."
                ));
            };
            let actor = s.actor.clone().ok_or_else(|| {
                format!("MCP server \"{server}\" disconnected; it reconnects on the next call")
            })?;
            (actor, t.original.clone())
        };
        let (otx, orx) = oneshot::channel();
        let id = self.inner.gen_seq.fetch_add(1, Ordering::SeqCst) + 100_000_000;
        let params = json!({ "name": original, "arguments": args });
        actor
            .send(ActorCmd::Request {
                id,
                params,
                reply: otx,
            })
            .map_err(|_| {
                format!("MCP server \"{server}\" disconnected; it reconnects on the next call")
            })?;
        match tokio::time::timeout(Duration::from_secs(cfg.timeout_secs), orx).await {
            Ok(Ok(Ok(result))) => Ok(map_call_result(server, &original, &result)),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(format!("MCP server \"{server}\" disconnected mid-call")),
            Err(_) => Err(format!(
                "MCP tool \"{server}/{original}\" timed out after {}s",
                cfg.timeout_secs
            )),
        }
    }

    /// Closes every connection (session shutdown): the actors end their
    /// transports (stdio: stdin close, then SIGTERM via kill-on-drop;
    /// http: best-effort DELETE) and report done.
    pub async fn close(&self) {
        if !self
            .inner
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }
        let dones: Vec<Option<oneshot::Receiver<()>>> = {
            let mut g = self.inner.servers.lock().unwrap();
            for s in g.values_mut() {
                s.state = ConnState::Closed;
                if let Some(tx) = s.actor.take() {
                    let _ = tx.send(ActorCmd::Close);
                }
            }
            g.values_mut().map(|s| s.done.take()).collect()
        };
        for done in dones.into_iter().flatten() {
            let _ = tokio::time::timeout(Duration::from_secs(3), done).await;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.servers.lock().unwrap().is_empty()
    }

    /// Runtime registration of a server (a user extension's
    /// `pi.registerMcpServer`, forwarded by the sidecar — D13 final form,
    /// E3S5 follow-up): the core hosts it exactly like a configured
    /// server (same stdio/streamable-HTTP transports, naming, exposure,
    /// native execution). `entry` is the mcp.json entry shape (the
    /// sidecar sends the upstream-validated config). Typed errors: a
    /// closed host, a duplicate name (v0: no replace — the sidecar's
    /// registry enforces the upstream ownership/clash rules), an
    /// `oauth`/`auth` entry (D17 — the core cannot authenticate it, no
    /// host remains), or an invalid entry. A server added before
    /// `start()` connects there; one added after connects at once.
    pub fn register_server(
        &self,
        name: &str,
        entry: &Value,
        extension_path: Option<&str>,
    ) -> Result<(), String> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err("the MCP host is closed (session shutdown)".into());
        }
        // One synthetic config map: reuses the configured-server parse /
        // validation exactly (incl. the D17 oauth/auth skip, which here
        // surfaces as the single warning = the typed error).
        let (servers, warnings) = parse_servers(Some(&json!({ "mcpServers": { name: entry } })));
        let mut cfg = servers
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| {
                warnings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("mcp server \"{name}\": invalid entry"))
            })?;
        cfg.extension_path = extension_path.map(str::to_string);
        let pump_running = self.inner.update_rx.lock().unwrap().is_none();
        {
            let mut g = self.inner.servers.lock().unwrap();
            if g.contains_key(name) {
                return Err(format!("mcp server \"{name}\" is already registered"));
            }
            let state = if cfg.enabled {
                ConnState::Starting
            } else {
                ConnState::Disabled
            };
            g.insert(
                name.to_string(),
                Server {
                    cfg,
                    state,
                    error: None,
                    tools: Vec::new(),
                    gen: Gen(0),
                    actor: None,
                    done: None,
                },
            );
        }
        // The update pump is live (start() has run): connect now. Before
        // start() the session-start sweep picks the server up.
        if pump_running && self.server_enabled(name) {
            let _ = self.start_attempt(name);
        }
        Ok(())
    }

    /// Every registered server as the `mcp_servers_change` payload:
    /// `[{ name, config, extensionPath? }]` — the configured servers plus
    /// the extension-registered ones (the core's full set; the core is the
    /// single source of truth for MCP, D13).
    pub fn server_entries(&self) -> Vec<Value> {
        self.inner
            .servers
            .lock()
            .unwrap()
            .values()
            .map(|s| {
                let mut e = json!({ "name": s.cfg.name, "config": s.cfg.raw });
                if let Some(p) = &s.cfg.extension_path {
                    e["extensionPath"] = json!(p);
                }
                e
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn rpc_request(method: &str, params: Value, id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn rpc_notification(method: &str) -> Value {
    json!({ "jsonrpc": "2.0", "method": method })
}

/// A response's result, or the JSON-RPC error as a string.
fn rpc_result(msg: &Value) -> Result<Value, String> {
    if let Some(e) = msg.get("error") {
        Err(format!(
            "MCP error {}: {}",
            e.get("code").and_then(Value::as_i64).unwrap_or(-1),
            e.get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        ))
    } else {
        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// A `tools/list` result into `ServerTool`s.
fn parse_tools(result: &Value) -> Vec<ServerTool> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let original = t.get("name")?.as_str()?.to_string();
                    Some(ServerTool {
                        original,
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        title: t.get("title").and_then(Value::as_str).map(str::to_string),
                        input_schema: t.get("inputSchema").cloned().unwrap_or(json!({})),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// MCP `CallToolResult` into the model-facing content (upstream
/// `convertMcpResult`, v0: text + image blocks; other blocks become a
/// text placeholder). `isError` results are error results; an empty
/// error gets the upstream fallback line.
pub fn map_call_result(server: &str, tool: &str, result: &Value) -> (Value, bool) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut content: Vec<Value> = Vec::new();
    for b in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = b.get("text").and_then(Value::as_str) {
                    content.push(json!({"type": "text", "text": text}));
                }
            }
            Some("image") => {
                if let (Some(data), Some(mime)) = (
                    b.pointer("/data").and_then(Value::as_str),
                    b.pointer("/mimeType").and_then(Value::as_str),
                ) {
                    content.push(json!({"type": "image", "data": data, "mimeType": mime}));
                }
            }
            Some("resource_link") => {
                let uri = b.get("uri").and_then(Value::as_str).unwrap_or("");
                let title = b
                    .get("title")
                    .or_else(|| b.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                content
                    .push(json!({"type": "text", "text": format!("[Resource {uri} \"{title}\"]")}));
            }
            Some("resource") => {
                let uri = b
                    .pointer("/resource/uri")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                content.push(json!({"type": "text", "text": format!("[Binary resource {uri} (omitted by the core in v0)]")}));
            }
            Some(t) => {
                content.push(json!({"type": "text", "text": format!("[{t} content omitted by the core in v0]")}));
            }
            None => {}
        }
    }
    if content.is_empty() {
        // Upstream `toLlmContent` fallback: structured content as JSON.
        if let Some(sc) = result.get("structuredContent") {
            if !sc.is_null() {
                content.push(json!({"type": "text", "text": sc.to_string()}));
            }
        }
    }
    if is_error
        && content.iter().all(|c| {
            c.get("text")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
    {
        content.push(
            json!({"type": "text", "text": format!("MCP tool {server}/{tool} returned an error")}),
        );
    }
    (json!(content), is_error)
}

// ---------------------------------------------------------------------------
// stdio actor
// ---------------------------------------------------------------------------

/// One stdio server connection (upstream `StdioTransport` +
/// `McpServerConnection::connectOnce`). The actor reports its own state on
/// `update_tx`: `Failed` on a handshake error, `Connected` (with the
/// `tools/list` result) once the handshake lands — then it serves
/// One newline-delimited JSON-RPC message to the server's stdin.
async fn send_line(stdin: &mut tokio::process::ChildStdin, msg: &Value) -> Result<(), String> {
    let line = format!("{msg}\n");
    stdin
        .write_all(line.as_bytes())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `tools/call` requests until the server exits or the host closes, sending
/// `Tools` on a `tools_list_changed` refresh and `Disconnected` on the
/// transport drop (upstream: the next call reconnects with a fresh process;
/// no auto-restart).
async fn stdio_actor(
    name: &str,
    cfg: &ServerConfig,
    cwd: &Path,
    gen: Gen,
    mut cmd_rx: mpsc::UnboundedReceiver<ActorCmd>,
    update_tx: mpsc::UnboundedSender<ServerUpdate>,
    timeout: Duration,
) {
    let name = name.to_string();
    let Kind::Stdio {
        command,
        args,
        env,
        cwd: cfg_cwd,
    } = &cfg.kind
    else {
        unreachable!()
    };
    let spawn_cwd = cwd.join(cfg_cwd.as_deref().unwrap_or("."));
    let mut child = match tokio::process::Command::new(command)
        .args(args)
        .envs(env.iter())
        .current_dir(&spawn_cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = update_tx.send(ServerUpdate::Failed {
                gen,
                name: name.to_string(),
                error: format!("spawn \"{command}\": {e}"),
            });
            return;
        }
    };
    let stderr_tail: Arc<Mutex<Vec<u8>>> = Arc::default();
    if let Some(mut stderr) = child.stderr.take() {
        let tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut t = tail.lock().unwrap();
                        t.extend_from_slice(&buf[..n]);
                        if t.len() > STDERR_TAIL_CHARS {
                            let drop_n = t.len() - STDERR_TAIL_CHARS;
                            t.drain(..drop_n);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = update_tx.send(ServerUpdate::Failed {
                gen,
                name: name.to_string(),
                error: "no stdin for the server".into(),
            });
            return;
        }
    };
    let stdout = match child.stdout.take() {
        Some(o) => o,
        None => {
            let _ = update_tx.send(ServerUpdate::Failed {
                gen,
                name: name.to_string(),
                error: "no stdout from the server".into(),
            });
            return;
        }
    };
    // Reader task: newline-delimited JSON-RPC (upstream `JsonRpcLineReader`).
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Value>();
    {
        let tx = in_tx.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let mut br = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match br.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let t = line.trim();
                        if t.is_empty() {
                            continue;
                        }
                        if t.len() > MAX_MESSAGE_BYTES {
                            crate::corelog::line(&format!("[core] mcp \"{name}\": message exceeds {MAX_MESSAGE_BYTES} bytes; dropped"));
                            continue;
                        }
                        match serde_json::from_str::<Value>(t) {
                            Ok(v) => {
                                let _ = tx.send(v);
                            }
                            Err(_) => {
                                crate::corelog::line(&format!(
                                    "[core] mcp \"{name}\": malformed line dropped"
                                ));
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            drop(tx);
        });
    }
    // The reader owns the only send half now: with the original dropped, the
    // receiver sees EOF (None) as soon as the reader exits (server death).
    drop(in_tx);
    // Handshake: initialize -> notifications/initialized -> tools/list
    // (upstream `McpClient::connect` + `connectOnce`).
    let init_params = json!({
        "protocolVersion": LATEST_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "pi", "version": env!("CARGO_PKG_VERSION") },
    });
    let handshake = async {
        send_line(&mut stdin, &rpc_request("initialize", init_params, 1)).await?;
        let init = recv_response(&mut in_rx, &mut child, 1, timeout).await?;
        send_line(&mut stdin, &rpc_notification("notifications/initialized")).await?;
        Ok::<_, String>(init)
    }
    .await;
    let tools = match handshake {
        Err(e) => {
            let _ = update_tx.send(ServerUpdate::Failed {
                gen,
                name: name.to_string(),
                error: e,
            });
            return;
        }
        Ok(init) => {
            // Servers without the tools capability (prompts/resources only)
            // do not answer tools/list (upstream).
            if init
                .get("capabilities")
                .and_then(|c| c.get("tools"))
                .is_none()
            {
                Vec::new()
            } else {
                match async {
                    send_line(&mut stdin, &rpc_request("tools/list", json!({}), 2)).await?;
                    recv_response(&mut in_rx, &mut child, 2, timeout).await
                }
                .await
                {
                    Ok(res) => parse_tools(&res),
                    Err(e) => {
                        let _ = update_tx.send(ServerUpdate::Failed {
                            gen,
                            name: name.to_string(),
                            error: e,
                        });
                        return;
                    }
                }
            }
        }
    };
    let _ = update_tx.send(ServerUpdate::Connected {
        gen,
        name: name.to_string(),
        tools,
    });

    // Serving loop: tools/call requests + the tools_list_changed refresh,
    // until the host closes or the server exits.
    let mut next_id = 3u64;
    let mut pending: HashMap<u64, oneshot::Sender<Result<Value, String>>> = HashMap::new();
    let mut refresh: Option<u64> = None; // in-flight tools/list id
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(ActorCmd::Close) => break,
                Some(ActorCmd::Request { params, reply, .. }) => {
                    let id = next_id;
                    next_id += 1;
                    pending.insert(id, reply);
                    if let Err(e) = send_line(&mut stdin, &rpc_request("tools/call", params, id)).await {
                        if let Some(r) = pending.remove(&id) {
                            let _ = r.send(Err(e));
                        }
                        let _ = update_tx.send(ServerUpdate::Disconnected {
                            gen,
                            name: name.to_string(),
                            error: "connection closed".into(),
                        });
                        break;
                    }
                }
                // The host dropped the actor without Close (should not happen).
                None => break,
            },
            msg = in_rx.recv() => match msg {
                None => {
                    // stdout closed: the server exited (upstream onClose).
                    for (_, r) in pending.drain() {
                        let _ = r.send(Err("MCP server exited mid-call".into()));
                    }
                    let _ = update_tx.send(ServerUpdate::Disconnected {
                        gen,
                        name: name.to_string(),
                        error: "connection closed".into(),
                    });
                    break;
                }
                Some(v) => {
                    if let Some(id) = v.get("id").and_then(Value::as_u64) {
                        if v.get("method").is_none() {
                            // A response (the refresh's tools/list or a call).
                            if let Some(r) = pending.remove(&id) {
                                let _ = r.send(rpc_result(&v));
                            } else if refresh == Some(id) {
                                refresh = None;
                                if let Ok(res) = rpc_result(&v) {
                                    let tools = parse_tools(&res);
                                    let _ = update_tx.send(ServerUpdate::Tools {
                                        gen,
                                        name: name.to_string(),
                                        tools,
                                    });
                                }
                            }
                            continue;
                        }
                    }
                    if v.get("method").and_then(Value::as_str)
                        == Some("notifications/tools_list_changed")
                        && refresh.is_none()
                    {
                        let id = next_id;
                        next_id += 1;
                        refresh = Some(id);
                        let _ = send_line(&mut stdin, &rpc_request("tools/list", json!({}), id)).await;
                    }
                    // Other notifications: ignored in v0.
                }
            }
        }
    }
    // Drop the child guard -> kill_on_drop reaps the process (dead or killed).
    let _ = (stderr_tail, child, stdin);
}

/// Waits for the response to `want_id` on the message stream; the child
/// dying (or the stream ending) is an error.
async fn recv_response(
    in_rx: &mut mpsc::UnboundedReceiver<Value>,
    child: &mut tokio::process::Child,
    want_id: u64,
    timeout: Duration,
) -> Result<Value, String> {
    let res = tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                msg = in_rx.recv() => match msg {
                    None => return Err("MCP server closed the connection during connect".into()),
                    Some(v) => {
                        if v.get("id").and_then(Value::as_u64) == Some(want_id)
                            && v.get("method").is_none()
                        {
                            return rpc_result(&v);
                        }
                        // A stray notification before the response: ignore.
                    }
                },
                _ = child.wait() => return Err("MCP server exited during connect".into()),
            }
        }
    })
    .await;
    match res {
        Ok(r) => r,
        Err(_) => Err(format!(
            "MCP server timed out during connect ({}s)",
            timeout.as_secs()
        )),
    }
}

// ---------------------------------------------------------------------------
// http actor
// ---------------------------------------------------------------------------

/// One HTTP server connection (upstream `StreamableHttpTransport` +
/// connect). The actor reports its own state on `update_tx`: `Failed`
/// (transient errors retried with upstream `[250, 1000]` delays first),
/// then `Connected` — and serves `tools/call` requests until Close
/// or a dropped session (404 → `Disconnected`; the next call reconnects).
async fn http_actor(
    name: &str,
    cfg: &ServerConfig,
    gen: Gen,
    mut cmd_rx: mpsc::UnboundedReceiver<ActorCmd>,
    update_tx: mpsc::UnboundedSender<ServerUpdate>,
    timeout: Duration,
) {
    let Kind::Http { url, headers } = &cfg.kind else {
        unreachable!()
    };
    let client = reqwest::Client::new();
    // Upstream retry: initial attempt + one retry per delay ([250, 1000]).
    let mut error: Option<String> = None;
    let mut tools = Vec::new();
    for (attempt, delay) in [None, Some(250u64), Some(1_000)].into_iter().enumerate() {
        if let Some(d) = delay {
            tokio::time::sleep(Duration::from_millis(d)).await;
        }
        let (t, _instructions, err) = http_handshake(&client, url, headers, timeout).await;
        error = err;
        let transient = error
            .as_deref()
            .is_some_and(|e| e.starts_with("transient:"));
        error = error.map(|e| e.strip_prefix("transient: ").unwrap_or(&e).to_string());
        if error.is_none() {
            tools = t;
            break;
        }
        if !transient || attempt == 2 {
            break;
        }
    }
    if let Some(e) = error {
        let _ = update_tx.send(ServerUpdate::Failed {
            gen,
            name: name.to_string(),
            error: e,
        });
        return;
    }
    let _ = update_tx.send(ServerUpdate::Connected {
        gen,
        name: name.to_string(),
        tools,
    });
    // Serving loop: one request at a time (the agent loop is sequential).
    let mut session: Option<String> = None;
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            ActorCmd::Close => break,
            ActorCmd::Request { id, params, reply } => {
                let msg = rpc_request("tools/call", params, id);
                match http_rpc(&client, url, headers, &mut session, timeout, &msg, false).await {
                    Ok(Some(result)) => {
                        let _ = reply.send(rpc_result(&result));
                    }
                    Ok(None) => {
                        let _ = reply.send(Err("MCP server returned no response".into()));
                    }
                    Err(HttpFail {
                        msg, disconnected, ..
                    }) => {
                        let _ = reply.send(Err(msg));
                        if disconnected {
                            let _ = update_tx.send(ServerUpdate::Disconnected {
                                gen,
                                name: name.to_string(),
                                error: "session expired".into(),
                            });
                            break;
                        }
                    }
                }
            }
        }
    }
    // Best-effort session close (upstream transport close: DELETE).
    let _ = http_delete(&client, url, headers, &session, Duration::from_secs(2)).await;
}

struct HttpFail {
    msg: String,
    /// The server dropped the session (404): reconnect on the next call.
    disconnected: bool,
}

/// One streamable-HTTP JSON-RPC exchange. Returns the full response
/// message (the caller extracts `result`), or a typed failure.
async fn http_rpc(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    session: &mut Option<String>,
    timeout: Duration,
    msg: &Value,
    is_notification: bool,
) -> Result<Option<Value>, HttpFail> {
    let is_request = !is_notification;
    let mut req = client
        .post(url)
        .timeout(timeout)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            if is_request {
                "application/json, text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(msg.to_string());
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s.as_str());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Upstream `isTransientError`: a network-level failure (fetch
            // TypeError) is transient — retry at connect time.
            return Err(HttpFail {
                msg: format!("transient: MCP HTTP request failed: {e}"),
                disconnected: false,
            });
        }
    };
    if let Some(s) = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
    {
        *session = Some(s.to_string());
    }
    let status = resp.status();
    if status == reqwest::StatusCode::NO_CONTENT || status.as_u16() == 202 {
        return Ok(None); // notification acknowledged
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(HttpFail {
            msg: format!(
                "MCP HTTP server answered {status}: sign-in (OAuth) is not supported by the core yet (E3S4)"
            ),
            disconnected: false,
        });
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(HttpFail {
            msg: "MCP HTTP session expired (404); reconnecting".into(),
            disconnected: true,
        });
    }
    if status.as_u16() == 405 {
        return Err(HttpFail {
            msg: format!("MCP HTTP server does not support streamable HTTP (405 on POST {url})"),
            disconnected: false,
        });
    }
    if status.as_u16() == 408
        || status.as_u16() == 429
        || (status.is_server_error() && status.as_u16() != 501)
    {
        let body = resp.text().await.unwrap_or_default();
        return Err(HttpFail {
            msg: format!("transient: MCP HTTP error {status}: {}", clip(&body, 200)),
            disconnected: false,
        });
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(HttpFail {
            msg: format!("MCP HTTP error {status}: {}", clip(&body, 200)),
            disconnected: false,
        });
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if ct.starts_with("text/event-stream") {
        // SSE: read until the JSON-RPC response for this message lands.
        // SseReader keeps its own line buffer — feed ONLY the new chunk
        // (feeding the accumulated buffer would double-process lines).
        let mut stream = resp.bytes_stream();
        let mut reader = SseReader::new();
        let mut bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            bytes += chunk.len();
            if bytes > MAX_MESSAGE_BYTES {
                return Err(HttpFail {
                    msg: "MCP SSE stream exceeds the message limit".into(),
                    disconnected: false,
                });
            }
            let text = String::from_utf8_lossy(&chunk).into_owned();
            if let Some(v) = reader.feed(&text) {
                return Ok(Some(v));
            }
        }
        Err(HttpFail {
            msg: "MCP SSE stream ended without a response".into(),
            disconnected: false,
        })
    } else {
        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) => {
                return Err(HttpFail {
                    msg: format!("MCP HTTP response read failed: {e}"),
                    disconnected: false,
                })
            }
        };
        if body.trim().is_empty() {
            return Ok(None);
        }
        match serde_json::from_str::<Value>(&body) {
            Ok(v) => Ok(Some(v)),
            Err(e) => Err(HttpFail {
                msg: format!("MCP HTTP response is not JSON-RPC: {e}"),
                disconnected: false,
            }),
        }
    }
}

/// Best-effort session DELETE (upstream transport close).
async fn http_delete(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    session: &Option<String>,
    timeout: Duration,
) -> Result<(), String> {
    let mut req = client
        .delete(url)
        .timeout(timeout)
        .header(reqwest::header::ACCEPT, "application/json");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s.as_str());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let _ = resp.bytes().await;
    Ok(())
}

/// The initialize handshake: initialize + notifications/initialized +
/// tools/list. The session header is captured for later requests.
async fn http_handshake(
    client: &reqwest::Client,
    url: &str,
    headers: &BTreeMap<String, String>,
    timeout: Duration,
) -> (Vec<ServerTool>, Option<String>, Option<String>) {
    let mut session: Option<String> = None;
    let init_msg = rpc_request(
        "initialize",
        json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "pi", "version": env!("CARGO_PKG_VERSION") },
        }),
        1,
    );
    let init = match http_rpc(
        client,
        url,
        headers,
        &mut session,
        timeout,
        &init_msg,
        false,
    )
    .await
    {
        Ok(Some(r)) => match rpc_result(&r) {
            Ok(res) => res,
            Err(e) => return (Vec::new(), None, Some(e)),
        },
        Ok(None) => {
            return (
                Vec::new(),
                None,
                Some("MCP HTTP: no initialize response".into()),
            )
        }
        Err(f) => return (Vec::new(), None, Some(f.msg)),
    };
    if !init
        .get("protocolVersion")
        .and_then(Value::as_str)
        .is_some()
    {
        return (
            Vec::new(),
            None,
            Some("MCP server sent an invalid initialize result".into()),
        );
    }
    if let Err(f) = http_rpc(
        client,
        url,
        headers,
        &mut session,
        timeout,
        &rpc_notification("notifications/initialized"),
        true,
    )
    .await
    {
        return (Vec::new(), None, Some(f.msg));
    }
    let has_tools = init
        .get("capabilities")
        .and_then(|c| c.get("tools"))
        .is_some();
    let instructions = init
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !has_tools {
        return (Vec::new(), instructions, None);
    }
    let tools_msg = rpc_request("tools/list", json!({}), 2);
    match http_rpc(
        client,
        url,
        headers,
        &mut session,
        timeout,
        &tools_msg,
        false,
    )
    .await
    {
        Ok(Some(r)) => match rpc_result(&r) {
            Ok(res) => (parse_tools(&res), instructions, None),
            Err(e) => (Vec::new(), None, Some(e)),
        },
        Ok(None) => (
            Vec::new(),
            None,
            Some("MCP HTTP: no tools/list response".into()),
        ),
        Err(f) => (Vec::new(), None, Some(f.msg)),
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

// ---------------------------------------------------------------------------
// SSE (upstream `consumeSseStream`, v0 subset)
// ---------------------------------------------------------------------------

/// Incremental SSE event reader: `feed` with decoded chunks; returns
/// the JSON value of the first `message` event whose data parses as
/// JSON (other events are skipped). Line endings: LF, CRLF, lone CR.
pub struct SseReader {
    /// Incomplete trailing line carried over between feeds.
    buf: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseReader {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            event: None,
            data: Vec::new(),
        }
    }

    /// Feed one chunk; returns the last `message` event's JSON value, if
    /// the chunk completed one. Incomplete trailing lines stay buffered.
    /// Line endings: CRLF, LF, or a lone CR (SSE spec).
    pub fn feed(&mut self, text: &str) -> Option<Value> {
        self.buf.push_str(text);
        let input = std::mem::take(&mut self.buf);
        let mut rest = input.as_str();
        let mut last: Option<Value> = None;
        while let Some((i, c)) = rest
            .char_indices()
            .find(|(_, ch)| matches!(ch, '\n' | '\r'))
        {
            let line = &rest[..i];
            let after = &rest[i + c.len_utf8()..];
            // CRLF: the \n right after the \r is part of the ending.
            let next = if c == '\r' && after.starts_with('\n') {
                &after[1..]
            } else {
                after
            };
            if let Some(v) = self.process_line(line) {
                last = Some(v);
            }
            rest = next;
        }
        self.buf = rest.to_string();
        last
    }

    /// One SSE line; dispatches the event on a blank line.
    fn process_line(&mut self, line: &str) -> Option<Value> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment
        }
        let (field, value) = match line.find(':') {
            Some(i) => (
                &line[..i],
                line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..]),
            ),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {} // id/retry/unknown: v0 does not support resumption
        }
        None
    }

    fn dispatch(&mut self) -> Option<Value> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        let data = self.data.join("\n");
        self.data.clear();
        let event = self.event.take();
        // SSE default event name is "message"; MCP responses may omit it.
        if event.as_deref() == Some("message") || event.is_none() {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                return Some(v);
            }
        }
        None
    }
}

impl Default for SseReader {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(exposure: &str, tool_exposure: BTreeMap<String, String>) -> ServerConfig {
        ServerConfig {
            name: "s".into(),
            kind: Kind::Http {
                url: "http://127.0.0.1:1/m".into(),
                headers: BTreeMap::new(),
            },
            enabled: true,
            exposure: exposure.into(),
            tool_exposure,
            timeout_secs: 60,
            raw: Value::Null,
            extension_path: None,
        }
    }

    // --- naming ---

    #[test]
    fn namespace_replaces_dashes() {
        assert_eq!(mcp_namespace("my-server"), "mcp__my_server");
        assert_eq!(mcp_namespace("plain"), "mcp__plain");
    }

    #[test]
    fn tool_name_sanitizes_and_hashes() {
        assert_eq!(
            mcp_tool_name("my-server", "do-it", |_| false),
            "mcp__my_server__do_it"
        );
        // Over 64 chars: hash suffix, total length 64 (upstream sha256("s\0<tool>") prefix).
        let long = "a".repeat(80);
        let n = mcp_tool_name("s", &long, |_| false);
        assert_eq!(n.len(), 64);
        assert_eq!(
            n,
            "mcp__s__aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa_2b133101"
        );
        // Collision: is_taken forces the hash (upstream shape: truncated base + "_" + 8 hex).
        let n = mcp_tool_name("s", "t", |_| true);
        assert_eq!(n, "mcp__s__t_12dce344");
        // Same input, same hash (deterministic).
        assert_eq!(n, mcp_tool_name("s", "t", |_| true));
    }

    #[test]
    fn pattern_matching() {
        assert!(pattern_match("e*", "echo"));
        assert!(pattern_match("*o*", "echo"));
        assert!(pattern_match("e*o", "echo"));
        assert!(!pattern_match("e*", "counter"));
        assert!(pattern_match("a.b", "a.b")); // dot is literal
        assert!(!pattern_match("a.b", "axb"));
        assert!(pattern_match("*", "anything"));
    }

    // --- exposure ---

    #[test]
    fn exposure_exact_beats_pattern_beats_default() {
        let mut te: BTreeMap<String, String> = BTreeMap::new();
        te.insert("exact".into(), "direct".into());
        te.insert("p*".into(), "hidden".into());
        te.insert("zz".into(), "deferred".into());
        let c = cfg("codemode", te);
        assert_eq!(tool_exposure(&c, "exact"), "direct");
        assert_eq!(tool_exposure(&c, "p123"), "hidden");
        assert_eq!(tool_exposure(&c, "zz"), "deferred");
        assert_eq!(tool_exposure(&c, "other"), "codemode");
    }

    #[test]
    fn has_direct_tools_sees_overrides() {
        let mut te: BTreeMap<String, String> = BTreeMap::new();
        te.insert("one".into(), "direct".into());
        assert!(has_direct_tools(&cfg("codemode", te)));
        assert!(!has_direct_tools(&cfg("codemode", BTreeMap::new())));
        assert!(has_direct_tools(&cfg("direct", BTreeMap::new())));
    }

    // --- value resolution ---

    #[test]
    fn resolve_value_escapes_and_refs() {
        std::env::set_var("MCP_TEST_VAR_XYZ", "secret-value");
        let ctx = "test";
        assert_eq!(resolve_value(ctx, "plain").unwrap(), "plain");
        assert_eq!(
            resolve_value(ctx, "${MCP_TEST_VAR_XYZ}").unwrap(),
            "secret-value"
        );
        assert_eq!(
            resolve_value(ctx, "$MCP_TEST_VAR_XYZ").unwrap(),
            "secret-value"
        );
        assert_eq!(
            resolve_value(ctx, "pre${MCP_TEST_VAR_XYZ}post").unwrap(),
            "presecret-valuepost"
        );
        assert_eq!(resolve_value(ctx, "a$$b").unwrap(), "a$b");
        assert_eq!(resolve_value(ctx, "a$!b").unwrap(), "a!b");
        assert_eq!(resolve_value(ctx, "cost: $5").unwrap(), "cost: $5"); // bare $ + non-name
        assert_eq!(
            resolve_value(ctx, "${not a name}").unwrap(),
            "${not a name}"
        );
        assert!(resolve_value(ctx, "${MCP_TEST_VAR_UNSET_XYZ}").is_err());
        assert!(resolve_value(ctx, "!echo hi")
            .unwrap_err()
            .contains("!command"));
        std::env::remove_var("MCP_TEST_VAR_XYZ");
    }

    // --- parse_servers ---

    #[test]
    fn parse_stdio_server_defaults() {
        let (servers, warnings) = parse_servers(Some(&json!({
            "mcpServers": {
                "fixture": { "command": "node", "args": ["x.mjs"] }
            }
        })));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s.name, "fixture");
        assert_eq!(s.exposure, "codemode"); // upstream default
        assert_eq!(s.timeout_secs, 60);
        assert!(s.enabled);
        if let Kind::Stdio {
            command,
            args,
            env,
            cwd,
        } = &s.kind
        {
            assert_eq!(command, "node");
            assert_eq!(args, &["x.mjs".to_string()]);
            assert!(env.is_empty());
            assert!(cwd.is_none());
        } else {
            panic!("expected stdio");
        }
    }

    #[test]
    fn parse_http_server_with_headers_and_exposure() {
        std::env::set_var("MCP_TEST_HDR_XYZ", "tok-123");
        let (servers, warnings) = parse_servers(Some(&json!({
            "mcpServers": {
                "httptest": {
                    "url": "http://127.0.0.1:9/m",
                    "exposure": "direct",
                    "headers": { "Authorization": "${MCP_TEST_HDR_XYZ}" },
                    "timeout": 5
                }
            }
        })));
        std::env::remove_var("MCP_TEST_HDR_XYZ");
        assert!(warnings.is_empty(), "{warnings:?}");
        let s = &servers[0];
        assert_eq!(s.exposure, "direct");
        assert_eq!(s.timeout_secs, 5);
        if let Kind::Http { url, headers } = &s.kind {
            assert_eq!(url, "http://127.0.0.1:9/m");
            assert_eq!(
                headers.get("Authorization").map(String::as_str),
                Some("tok-123")
            );
        } else {
            panic!("expected http");
        }
    }

    #[test]
    fn parse_rejects_bad_entries_with_warnings() {
        let (servers, warnings) = parse_servers(Some(&json!({
            "mcpServers": {
                "both": { "command": "node", "url": "http://x/m" },
                "none": {},
                "badexp": { "command": "node", "exposure": "nope" },
                "oauth": { "url": "http://x/m", "oauth": {} },
                "badcmd": { "command": "" },
                "notstr": 1,
                "good": { "command": "node" }
            }
        })));
        assert_eq!(servers.len(), 1, "{servers:?}");
        assert_eq!(servers[0].name, "good");
        assert_eq!(warnings.len(), 6, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("both")));
        assert!(warnings.iter().any(|w| w.contains("none")));
        assert!(warnings.iter().any(|w| w.contains("badexp")));
        assert!(warnings.iter().any(|w| w.contains("oauth")));
        assert!(warnings.iter().any(|w| w.contains("badcmd")));
        assert!(warnings.iter().any(|w| w.contains("notstr")));
    }

    #[test]
    fn parse_env_command_ref_rejected() {
        let (servers, warnings) = parse_servers(Some(&json!({
            "mcpServers": {
                "cmd": { "command": "node", "env": { "K": "!echo x" } }
            }
        })));
        assert!(servers.is_empty());
        assert!(warnings[0].contains("!command"), "{warnings:?}");
    }

    // --- result mapping ---

    #[test]
    fn map_result_text_and_error() {
        let (content, is_error) = map_call_result(
            "s",
            "t",
            &json!({"content": [{"type": "text", "text": "echo:x"}], "isError": false}),
        );
        assert!(!is_error);
        assert_eq!(content, json!([{"type": "text", "text": "echo:x"}]));
        let (content, is_error) =
            map_call_result("s", "t", &json!({"content": [], "isError": true}));
        assert!(is_error);
        assert_eq!(
            content,
            json!([{"type": "text", "text": "MCP tool s/t returned an error"}])
        );
        // structuredContent fallback when no content blocks
        let (content, _) = map_call_result("s", "t", &json!({"structuredContent": {"count": 1}}));
        assert!(content[0]["text"].as_str().unwrap().contains("\"count\":1"));
    }

    // --- SSE ---

    #[test]
    fn sse_assembles_events() {
        let mut r = SseReader::new();
        assert!(r.feed("event: messa").is_none());
        assert!(r
            .feed("ge\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"")
            .is_none());
        assert_eq!(
            r.feed("result\":{\"ok\":true}}\n\n"),
            Some(json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}))
        );
    }

    #[test]
    fn sse_multi_data_lines_and_crlf() {
        let mut r = SseReader::new();
        // Multiple `data:` lines join with \n (CRLF endings here); the
        // joined value `{"a":\n1}` is legal JSON ({"a": 1}).
        let v = r.feed("data: {\"a\":\r\n");
        assert!(v.is_none());
        let v = r.feed("data: 1}\r\n\r\n");
        assert_eq!(v, Some(json!({"a": 1})));
        // A line without a field is not data (SSE spec) — ignored.
        let mut r = SseReader::new();
        assert_eq!(
            r.feed("data: {\"x\":1}\r\njunkline\r\n\r\n"),
            Some(json!({"x": 1}))
        );
        // A non-message event is skipped.
        let mut r = SseReader::new();
        assert_eq!(
            r.feed("event: other\ndata: {\"x\":1}\n\nevent: message\ndata: {\"y\":2}\n\n"),
            Some(json!({"y": 2}))
        );
        // Comments ignored.
        let mut r = SseReader::new();
        assert_eq!(r.feed(": hi\ndata: {\"z\":3}\n\n"), Some(json!({"z": 3})));
    }

    #[test]
    fn sse_split_crlf_across_feeds() {
        let mut r = SseReader::new();
        assert!(r.feed("data: {\"a\":1}\r").is_none());
        assert_eq!(r.feed("\n\n"), Some(json!({"a": 1})));
    }

    // --- host (no servers, no actors) ---

    #[tokio::test]
    async fn host_without_config_is_empty_and_close_is_a_noop() {
        let (host, warnings) = McpHost::new(None, Path::new("."));
        assert!(warnings.is_empty());
        assert!(host.is_empty());
        let host = Arc::new(host);
        host.await_direct().await;
        host.close().await;
        assert!(host.declared_tools().is_empty());
    }

    #[tokio::test]
    async fn host_disabled_server_is_never_connected() {
        let (host, _) = McpHost::new(
            Some(
                &json!({"mcpServers": {"off": {"command": "definitely-not-a-cmd", "enabled": false}}}),
            ),
            Path::new("."),
        );
        let host = Arc::new(host);
        // A call to a disabled server is a typed error, not a spawn.
        let err = host.call("off", "x", &json!({})).await.unwrap_err();
        assert!(err.contains("disabled"), "{err}");
        host.close().await;
    }

    #[tokio::test]
    async fn host_unknown_server_call_errors() {
        let (host, _) = McpHost::new(None, Path::new("."));
        let host = Arc::new(host);
        let err = host.call("nope", "x", &json!({})).await.unwrap_err();
        assert!(err.contains("unknown MCP server"), "{err}");
        host.close().await;
    }

    #[test]
    fn server_prefix_of_picks_longest_namespace() {
        let (host, _) = McpHost::new(
            Some(&json!({"mcpServers": {
                "a": {"command": "x"},
                "a_b": {"command": "y"},
                "other": {"url": "http://127.0.0.1:1/m"}
            }})),
            Path::new("."),
        );
        assert_eq!(
            host.server_prefix_of("mcp__a_b__tool"),
            Some(("a_b".into(), "tool".into()))
        );
        assert_eq!(
            host.server_prefix_of("mcp__a__tool"),
            Some(("a".into(), "tool".into()))
        );
        assert_eq!(
            host.server_prefix_of("mcp__other__x"),
            Some(("other".into(), "x".into()))
        );
        assert_eq!(host.server_prefix_of("read"), None);
    }

    // --- runtime registration (registerMcpServer forward, D13 follow-up) ---

    #[test]
    fn register_server_stdio_inserts_and_exposes_entry() {
        let (host, _) = McpHost::new(None, Path::new("."));
        let entry = json!({ "command": "node", "args": ["server.mjs"], "exposure": "direct" });
        host.register_server("ext", &entry, Some("/exts/fixture.ts"))
            .unwrap();
        let cfg = host.server_config("ext").expect("server not registered");
        assert!(matches!(cfg.kind, Kind::Stdio { .. }), "{:?}", cfg.kind);
        assert_eq!(cfg.exposure, "direct");
        assert_eq!(cfg.extension_path.as_deref(), Some("/exts/fixture.ts"));
        // The mcp_servers_change payload carries name + the raw entry + owner.
        let entries = host.server_entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0]["name"], "ext");
        assert_eq!(entries[0]["config"]["command"], "node");
        assert_eq!(entries[0]["extensionPath"], "/exts/fixture.ts");
    }

    #[test]
    fn register_server_http_inserts() {
        let (host, _) = McpHost::new(None, Path::new("."));
        host.register_server(
            "remote",
            &json!({ "url": "http://127.0.0.1:1/mcp", "headers": { "Authorization": "Bearer x" } }),
            None,
        )
        .unwrap();
        let cfg = host.server_config("remote").expect("server not registered");
        assert!(matches!(cfg.kind, Kind::Http { .. }), "{:?}", cfg.kind);
        // Configured servers carry no extensionPath (absent on the wire).
        assert!(host.server_entries()[0].get("extensionPath").is_none());
    }

    #[test]
    fn register_server_duplicate_is_typed_error() {
        let (host, _) = McpHost::new(None, Path::new("."));
        let entry = json!({ "command": "node" });
        host.register_server("dup", &entry, None).unwrap();
        let err = host
            .register_server("dup", &json!({ "command": "other" }), None)
            .unwrap_err();
        assert!(err.contains("already registered"), "{err}");
    }

    #[test]
    fn register_server_oauth_auth_is_typed_rejection() {
        // D17: the core cannot authenticate oauth/auth servers — a forwarded
        // registration is a typed error (the sidecar surfaces it to the
        // extension), not a silent inert entry.
        let (host, _) = McpHost::new(None, Path::new("."));
        let err = host
            .register_server(
                "o",
                &json!({ "url": "https://mcp.example.com", "oauth": {} }),
                None,
            )
            .unwrap_err();
        assert!(err.contains("D17"), "{err}");
        assert!(host.is_empty());
        let err = host
            .register_server(
                "a",
                &json!({ "url": "http://127.0.0.1:1/mcp", "auth": { "provider": "openai" } }),
                None,
            )
            .unwrap_err();
        assert!(err.contains("D17"), "{err}");
        assert!(host.is_empty());
    }

    #[test]
    fn register_server_invalid_entries_are_typed_errors() {
        let (host, _) = McpHost::new(None, Path::new("."));
        // No command and no url.
        let err = host
            .register_server("bad", &json!({ "exposure": "direct" }), None)
            .unwrap_err();
        assert!(err.contains("needs \"command\""), "{err}");
        // Both command and url.
        let err = host
            .register_server(
                "bad2",
                &json!({ "command": "node", "url": "http://127.0.0.1:1" }),
                None,
            )
            .unwrap_err();
        assert!(err.contains("exactly one"), "{err}");
        // Bad exposure value.
        let err = host
            .register_server(
                "bad3",
                &json!({ "command": "node", "exposure": "loud" }),
                None,
            )
            .unwrap_err();
        assert!(err.contains("exposure"), "{err}");
        assert!(host.is_empty());
    }

    #[tokio::test]
    async fn register_server_after_close_is_typed_error() {
        let (host, _) = McpHost::new(None, Path::new("."));
        let host = Arc::new(host);
        host.close().await;
        let err = host
            .register_server("late", &json!({ "command": "node" }), None)
            .unwrap_err();
        assert!(err.contains("closed"), "{err}");
    }
}
