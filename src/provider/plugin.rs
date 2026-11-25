//! Provider plugins (E4S1/WP-1501): the stable C ABI vtable + directory
//! loader for provider libraries (D16, option 1 — the ADR 0005/0006
//! in-process cdylib pattern).
//!
//! A plugin is a cdylib (`.so`/`.dylib`/`.dll`) exposing one entry symbol,
//! `pi_provider_plugin_init() -> *const PiProviderVtable` (NULL = not a
//! plugin). All payloads cross the boundary as NUL-terminated UTF-8 JSON
//! strings (or plain scalars); the full contract — vtable shapes, JSON
//! schemas, the string-ownership convention, error convention — is
//! [`docs/provider-plugin-abi.md`](../../docs/provider-plugin-abi.md).
//!
//! **Directories** (scanned at startup, in this order):
//! 1. the **bundled dir** — `<exe dir>/providers/` (shipped with the binary);
//! 2. the **user dir** — `<agentDir>/providers/`.
//!
//! A name collision: the later dir wins (user shadows bundled, logged).
//!
//! **Failure semantics** (E4S1 acceptance): a failing plugin (unreadable
//! file, missing entry symbol, NULL/`{"error"}` returns, `abi_version`
//! mismatch, bad `list_models` JSON, a plugin panic) is a TYPED LOG LINE and
//! the run continues with the remaining providers.
//!
//! **Trust**: plugins run in-process with the same trust level as
//! compiled-in builtins (the user's own dirs — no sandbox in v0; a faulty
//! plugin can take down the run, the same accepted risk as a builtin,
//! ADR 0006). Every vtable call is wrapped in `catch_unwind` (the panic
//! safety net of ADR 0006); memory faults are not catchable.
//!
//! **Credentials**: plugins never touch `auth.json` or the login UX. The
//! core resolves the credential ([`resolve_plugin_credential`]) and passes
//! it in the stream request; the core drives the login handshake
//! (`auth_start`/`auth_status`) and stores the returned credential
//! (`crate::auth::save_credential`).

use crate::model::{AssistantTurn, ModelDriver, StreamDelta};
use crate::provider::ProviderDriver;
use crate::registry::Model;
use anyhow::{Context, Result};
use serde_json::Value;
use std::ffi::{CStr, CString};
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

/// The core's provider plugin ABI version. A plugin reporting any other
/// value is skipped with a typed log line (the run continues).
pub const ABI_VERSION: u32 = 1;

/// The entry symbol every plugin cdylib must export.
pub const ENTRY_SYMBOL: &str = "pi_provider_plugin_init";

/// The C ABI vtable (ABI v1). Field order and signatures are the contract —
/// see `docs/provider-plugin-abi.md`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PiProviderVtable {
    /// The plugin's ABI version (checked against [`ABI_VERSION`]).
    pub abi_version: extern "C" fn() -> u32,
    /// The provider name (static, NUL-terminated; the core never frees it).
    pub name: extern "C" fn() -> *const u8,
    /// JSON array of registry-shaped model entries (see the ABI doc).
    pub list_models: extern "C" fn() -> *const u8,
    /// Starts a stream; returns `{"handle": "..."}` or `{"error": "..."}`.
    pub stream_open: extern "C" fn(*const u8) -> *const u8,
    /// The next event JSON, or NULL once the stream has ended.
    pub stream_next: extern "C" fn(*const u8) -> *const u8,
    /// Frees the plugin-side stream state (idempotent).
    pub stream_close: extern "C" fn(*const u8),
    /// Starts the login handshake; returns the first status JSON.
    pub auth_start: extern "C" fn(*const u8) -> *const u8,
    /// Polls the login handshake; returns a status JSON.
    pub auth_status: extern "C" fn(*const u8) -> *const u8,
    /// Releases plugin state at shutdown (idempotent).
    pub dispose: extern "C" fn(),
}

type InitFn = unsafe extern "C" fn() -> *const PiProviderVtable;

/// A loaded provider plugin (the `Library` stays alive for the process —
/// dropping it would unload the code the function pointers live in).
pub struct LoadedPlugin {
    pub name: String,
    /// `list_models` entries, registry-shaped, `provider` = the vtable name.
    pub models: Vec<Model>,
    pub path: PathBuf,
    /// Holds the dlopen handle — dropping the plugin would unload the code
    /// the vtable function pointers live in. Never read (the keep-alive is
    /// the point).
    #[allow(dead_code)]
    lib: libloading::Library,
    vt: PiProviderVtable,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("name", &self.name)
            .field("models", &self.models.len())
            .field("path", &self.path)
            .finish()
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        // Best effort: a panicking dispose must not mask the real shutdown.
        let _ = catch_unwind(AssertUnwindSafe(|| (self.vt.dispose)()));
    }
}

impl LoadedPlugin {
    fn call_str(&self, f: extern "C" fn(*const u8) -> *const u8, req: &str) -> Result<String> {
        // The ABI requires NUL-terminated UTF-8; a `String` is not
        // NUL-terminated, so hand the plugin a `CString`'s buffer.
        let req_c = CString::new(req).map_err(|_| {
            anyhow::anyhow!(
                "provider plugin {}: request contains an interior NUL",
                self.name
            )
        })?;
        // SAFETY: `req_c` is a valid NUL-terminated UTF-8 string for the
        // call; the returned pointer is valid until the next vtable call.
        // `as_bytes_with_nul().as_ptr()` is the platform-independent form
        // (c_char is i8 or u8 depending on the target).
        let ptr = catch_unwind(AssertUnwindSafe(|| f(req_c.as_bytes_with_nul().as_ptr())))
            .map_err(|_| anyhow::anyhow!("provider plugin {} panicked", self.name))?;
        cstr_to_string(ptr)
            .map_err(|e| anyhow::anyhow!("provider plugin {name}: {e}", name = self.name))
    }

    /// `auth_start` / `auth_status` with the v0 empty request `{}`.
    pub fn auth(&self, start: bool, req: &str) -> Result<AuthStatus> {
        let f = if start {
            self.vt.auth_start
        } else {
            self.vt.auth_status
        };
        let s = self.call_str(f, req)?;
        let v: Value = serde_json::from_str(&s).with_context(|| {
            format!(
                "provider plugin {}: auth status is not valid JSON",
                self.name
            )
        })?;
        parse_auth_status(&v, &self.name)
    }
}

// ---------------------------------------------------------------------------
// Boundary helpers

/// A NUL-terminated UTF-8 string from the plugin into an owned `String`
/// (the core copies before the next vtable call — the ownership convention
/// of the ABI doc).
fn cstr_to_string(ptr: *const u8) -> Result<String> {
    // SAFETY: vtable contracts — every non-NULL return is a valid
    // NUL-terminated UTF-8 string for the duration of the call. The
    // `as *const _` reinterprets to `*const c_char` (1-byte type; the
    // placeholder keeps it correct on every platform).
    let s = unsafe { CStr::from_ptr(ptr as *const _) };
    s.to_str()
        .map(str::to_string)
        .map_err(|_| anyhow::anyhow!("non-UTF-8 string from the plugin"))
}

/// Parses a plugin JSON return, converting the `{"error": "..."}`
/// convention into a typed error (the ABI doc's single error convention).
fn plugin_json(s: &str, name: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(s).with_context(|| "provider plugin {name}: bad JSON")?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        anyhow::bail!("provider plugin {name}: {e}");
    }
    Ok(v)
}

/// A login status (`docs/provider-plugin-abi.md`, `auth_start`/`auth_status`).
#[derive(Debug)]
pub enum AuthStatus {
    /// The handshake is in flight; `url` is where the user signs in
    /// (the core prints it and optionally opens a browser).
    Pending { url: Option<String> },
    /// The credential material for the core to store in auth.json
    /// (upstream credential shape, e.g. `{"type":"api_key","key":...}`).
    Done { credential: Value },
    /// The handshake failed (plugin-reported).
    Failed { error: String },
}

fn parse_auth_status(v: &Value, name: &str) -> Result<AuthStatus> {
    match v.get("state").and_then(Value::as_str) {
        Some("pending") => Ok(AuthStatus::Pending {
            url: v.get("url").and_then(Value::as_str).map(str::to_string),
        }),
        Some("done") => Ok(AuthStatus::Done {
            credential: v
                .get("credential")
                .cloned()
                .with_context(|| "provider plugin {name}: auth done without `credential`")?,
        }),
        Some("failed") => Ok(AuthStatus::Failed {
            error: v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown login failure")
                .to_string(),
        }),
        Some(other) => anyhow::bail!("provider plugin {name}: unknown auth state {other:?}"),
        None => anyhow::bail!("provider plugin {name}: auth status missing `state`"),
    }
}

// ---------------------------------------------------------------------------
// Directory loader

/// The provider plugins loaded at startup (empty set = no plugin dirs).
#[derive(Debug, Default, Clone)]
pub struct PluginSet {
    by_name: std::collections::BTreeMap<String, Arc<LoadedPlugin>>,
}

fn plugin_exts() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["dll"]
    } else if cfg!(target_os = "macos") {
        vec!["dylib"]
    } else {
        vec!["so"]
    }
}

impl PluginSet {
    /// Scans the bundled dir (`<exe dir>/providers/`) then the user dir
    /// (`<agentDir>/providers/`); a name collision — the user dir wins and
    /// the override is logged. Every failure is a typed log line (the run
    /// continues, E4S1 acceptance).
    pub fn load(agent_dir: &Path) -> PluginSet {
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("providers")));
        let user = agent_dir.join("providers");
        Self::load_dirs(bundled.as_deref(), Some(&user))
    }

    /// The explicit-dir variant (the loader logic; unit tests + `load`).
    pub fn load_dirs(bundled: Option<&Path>, user: Option<&Path>) -> PluginSet {
        let mut set = PluginSet::default();
        for (dir, source) in bundled
            .map(|d| (d, "bundled"))
            .into_iter()
            .chain(user.map(|d| (d, "user")))
        {
            if !dir.is_dir() {
                continue; // no dir = no plugins (a no-op, not an error)
            }
            let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension()
                            .is_some_and(|ext| plugin_exts().contains(&ext.to_str().unwrap_or("")))
                    })
                    .collect(),
                Err(e) => {
                    crate::corelog::line(&format!(
                        "[core] provider plugins ({source} dir {}): unreadable ({e}) — skipped",
                        dir.display()
                    ));
                    continue;
                }
            };
            files.sort();
            for path in files {
                match Self::load_one(&path) {
                    Ok(pl) => {
                        let name = pl.name.clone();
                        crate::corelog::line(&format!(
                            "[core] provider plugin loaded: {name} ({} models) from {}",
                            pl.models.len(),
                            path.display()
                        ));
                        let replaced = set.by_name.insert(name.clone(), Arc::new(pl));
                        if let Some(prev) = replaced {
                            crate::corelog::line(&format!(
                                "[core] provider plugin override: {name} ({}) shadows the {} plugin {}",
                                path.display(),
                                if source == "user" { "bundled" } else { "earlier" },
                                prev.path.display()
                            ));
                        }
                    }
                    Err(e) => crate::corelog::line(&format!(
                        "[core] provider plugin {} failed to load: {e} — skipped (the run continues)",
                        path.display()
                    )),
                }
            }
        }
        set
    }

    /// dlopen + entry symbol + `abi_version` check + `name` + `list_models`.
    fn load_one(path: &Path) -> Result<LoadedPlugin> {
        // SAFETY: `path` is a valid shared-library path; a failure (unreadable
        // file, truncated/corrupt image, missing platform support) is a typed
        // error — the plugin is skipped, the run continues.
        let lib = unsafe { libloading::Library::new(path) }
            .with_context(|| format!("dlopen {}", path.display()))?;
        let init: InitFn = *unsafe {
            lib.get(ENTRY_SYMBOL.as_bytes()).with_context(|| {
                format!(
                    "entry symbol `{ENTRY_SYMBOL}` missing from {}",
                    path.display()
                )
            })?
        };
        // SAFETY: the entry symbol contract — returns a valid vtable or NULL.
        let vt = unsafe { init() };
        if vt.is_null() {
            anyhow::bail!("`{ENTRY_SYMBOL}` returned NULL");
        }
        // SAFETY: the returned pointer is a valid, stable vtable (ABI doc).
        let vt = unsafe { &*vt };
        let version = catch_unwind(AssertUnwindSafe(|| (vt.abi_version)()))
            .map_err(|_| anyhow::anyhow!("the plugin's abi_version panicked"))?;
        if version != ABI_VERSION {
            anyhow::bail!("abi_version mismatch (plugin {version}, core {ABI_VERSION})");
        }
        let name_ptr = catch_unwind(AssertUnwindSafe(|| (vt.name)()))
            .map_err(|_| anyhow::anyhow!("the plugin's name panicked"))?;
        let name = cstr_to_string(name_ptr)?;
        if !is_valid_provider_name(&name) {
            anyhow::bail!("invalid provider name {name:?}");
        }
        let models_ptr = catch_unwind(AssertUnwindSafe(|| (vt.list_models)()))
            .map_err(|_| anyhow::anyhow!("the plugin's list_models panicked"))?;
        let models_raw = cstr_to_string(models_ptr)?;
        let arr: Value =
            serde_json::from_str(&models_raw).with_context(|| "list_models is not valid JSON")?;
        let entries = arr.as_array().context("list_models must be a JSON array")?;
        let mut models = Vec::with_capacity(entries.len());
        for e in entries {
            let mut m: Model = serde_json::from_value(e.clone())
                .with_context(|| "list_models entry is not registry-shaped")?;
            // The vtable name is authoritative (the entry's `provider` is
            // informational and may be absent on the wire).
            m.provider = name.clone();
            // Upstream model-config defaults (the E3S1/L6 rule): the
            // compaction window needs both.
            m.context_window = m.context_window.or(Some(128_000));
            m.max_tokens = m.max_tokens.or(Some(16_384));
            models.push(m);
        }
        Ok(LoadedPlugin {
            name,
            models,
            path: path.to_path_buf(),
            lib,
            vt: *vt,
        })
    }

    /// The plugin registered under `name`, if any.
    pub fn provider(&self, name: &str) -> Option<&Arc<LoadedPlugin>> {
        self.by_name.get(name)
    }

    /// Every plugin model (registry merge input: the bundled catalog wins
    /// per model id, the same v0 limit as models.json).
    pub fn all_models(&self) -> Vec<Model> {
        self.by_name
            .values()
            .flat_map(|p| p.models.iter().cloned())
            .collect()
    }

    /// Builds the plugin driver for `plugin`/`model` (typed errors: unknown
    /// model, missing credential). The plugin streams through the vtable —
    /// never the bridge (D11/D15-style core ownership).
    pub fn driver(
        &self,
        plugin: &Arc<LoadedPlugin>,
        model: Option<&str>,
        agent_dir: &Path,
    ) -> Result<ProviderDriver> {
        let m = model
            .ok_or_else(|| anyhow::anyhow!("--provider {} requires --model <id>", plugin.name))?;
        let model = plugin
            .models
            .iter()
            .find(|x| x.id == m)
            .cloned()
            .ok_or_else(|| {
                let available: Vec<&str> = plugin.models.iter().map(|x| x.id.as_str()).collect();
                anyhow::anyhow!(
                    "model {m} is not provided by provider plugin {} (available: {})",
                    plugin.name,
                    available.join(", ")
                )
            })?;
        let credential = resolve_plugin_credential(&plugin.name, agent_dir)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no credentials for provider {} — run `pi-rust auth login {}` (account sign-in) or `pi-rust auth login {} --api-key`",
                    plugin.name, plugin.name, plugin.name
                )
            })?;
        Ok(ProviderDriver::Plugin(Arc::new(PluginDriver {
            plugin: Arc::clone(plugin),
            model,
            credential,
        })))
    }
}

/// Provider names are kebab-case identifiers (upstream provider ids).
fn is_valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The credential material for a plugin stream (core-owned resolution):
/// a stored `api_key` credential (with `${ENV}` expansion) beats a stored
/// oauth credential (v0: the access token as-is — per-plugin refresh lands
/// with E4S2); no env-var table for plugin providers — the login flows
/// store the credential. `None` = nothing stored (the caller types the
/// error).
pub fn resolve_plugin_credential(name: &str, agent_dir: &Path) -> Result<Option<Value>> {
    if crate::auth::has_api_key_credential(agent_dir, name) {
        let key = crate::auth::resolve_api_key(name, agent_dir)?;
        return Ok(Some(serde_json::json!({ "type": "api_key", "key": key })));
    }
    if let Some(cred) = crate::oauth::stored(agent_dir, name)? {
        return Ok(Some(
            serde_json::json!({ "type": "oauth", "access": cred.access }),
        ));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// The plugin driver (the ProviderDriver::Plugin seam)

/// One live plugin stream: the vtable is driven from a blocking task
/// (`stream_next` blocks per event by contract) so the agent loop's async
/// executor is never parked.
pub struct PluginDriver {
    pub plugin: Arc<LoadedPlugin>,
    pub model: Model,
    pub credential: Value,
}

impl std::fmt::Debug for PluginDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginDriver")
            .field("plugin", &self.plugin.name)
            .field("model", &self.model.id)
            .finish()
    }
}

/// One message from the blocking FFI task to the async future.
enum DriveMsg {
    Delta(StreamDelta),
    Done(Result<AssistantTurn, String>),
}

impl ModelDriver for PluginDriver {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        let plugin = Arc::clone(&self.plugin);
        // The stream request (ABI doc): the registry model (camelCase),
        // the LLM conversation, and the core-resolved credential.
        let request = serde_json::json!({
            "model": self.model,
            "messages": convo,
            "credential": self.credential,
        })
        .to_string();
        let mut on_delta = on_delta;
        Box::pin(async move {
            // The vtable is driven from a blocking task (`stream_next`
            // blocks per event by contract) so the agent loop's async
            // executor is never parked; deltas cross as channel messages.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::task::spawn_blocking(move || drive_stream(&plugin, &request, tx))
                .await
                .map_err(|e| anyhow::anyhow!("provider plugin task join failed: {e}"))?;
            while let Some(msg) = rx.recv().await {
                match msg {
                    DriveMsg::Delta(d) => on_delta(d),
                    DriveMsg::Done(r) => return r.map_err(|e| anyhow::anyhow!("{e}")),
                }
            }
            anyhow::bail!("the plugin stream ended without a result")
        })
    }
}

impl ModelDriver for Arc<PluginDriver> {
    fn complete<'a>(
        &'a self,
        convo: &'a [Value],
        on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantTurn>> + Send + 'a>> {
        self.as_ref().complete(convo, on_delta)
    }
}

/// The blocking FFI stream loop (runs on the blocking pool):
/// `stream_open` → `stream_next`* → `stream_close`. Deltas are forwarded
/// over `tx`; the final `done` event's `turn` (or the error) is the last
/// message, always.
fn drive_stream(
    plugin: &LoadedPlugin,
    request: &str,
    tx: tokio::sync::mpsc::UnboundedSender<DriveMsg>,
) {
    let on_delta = &mut |d: StreamDelta| {
        let _ = tx.send(DriveMsg::Delta(d));
    };
    let result = drive_stream_inner(plugin, request, on_delta);
    let _ = tx.send(DriveMsg::Done(result.map_err(|e| e.to_string())));
}

fn drive_stream_inner(
    plugin: &LoadedPlugin,
    request: &str,
    on_delta: &mut impl FnMut(StreamDelta),
) -> Result<AssistantTurn> {
    let open = plugin.call_str(plugin.vt.stream_open, request)?;
    let open: Value = plugin_json(&open, &plugin.name)?;
    let handle = open
        .get("handle")
        .and_then(Value::as_str)
        .with_context(|| "stream_open returned no `handle`")?
        .to_string();
    // NUL-terminated for every `stream_next`/`stream_close` call: a plain
    // `String` buffer is not NUL-terminated, the ABI requires it.
    let handle_c = CString::new(handle)
        .map_err(|_| anyhow::anyhow!("the plugin returned a handle with an interior NUL"))?;

    let mut text_full = String::new();
    let mut text_started = false;
    let mut turn: Option<Value> = None;
    loop {
        let raw = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `handle_c` is valid NUL-terminated UTF-8 for the call;
            // a NULL return ends the stream.
            (plugin.vt.stream_next)(handle_c.as_bytes_with_nul().as_ptr())
        }))
        .map_err(|_| anyhow::anyhow!("provider plugin {} panicked in stream_next", plugin.name))?;
        if raw.is_null() {
            break;
        }
        let ev_s = cstr_to_string(raw)
            .map_err(|e| anyhow::anyhow!("provider plugin {}: {e}", plugin.name))?;
        let ev = plugin_json(&ev_s, &plugin.name)?;
        // A `{"type":"error",...}` event is a plugin-reported wire failure.
        let etype = ev.get("type").and_then(Value::as_str);
        if etype == Some("error") {
            anyhow::bail!(
                "provider plugin {}: {}",
                plugin.name,
                ev.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown stream error")
            );
        }
        match etype {
            Some("delta") => {
                let d = ev.get("delta").and_then(Value::as_str).unwrap_or_default();
                if !text_started {
                    text_started = true;
                    on_delta(StreamDelta::TextStart { index: 0 });
                }
                text_full.push_str(d);
                on_delta(StreamDelta::TextDelta {
                    index: 0,
                    delta: d.to_string(),
                });
            }
            Some("done") => {
                if text_started {
                    on_delta(StreamDelta::TextEnd {
                        index: 0,
                        content: text_full.clone(),
                    });
                }
                turn = Some(
                    ev.get("turn")
                        .cloned()
                        .with_context(|| "the done event has no `turn`")?,
                );
                break;
            }
            Some(other) => {
                anyhow::bail!(
                    "provider plugin {}: unknown stream event type {other:?}",
                    plugin.name
                )
            }
            None => {
                anyhow::bail!(
                    "provider plugin {}: stream event missing `type`",
                    plugin.name
                )
            }
        }
    }
    // Best effort: a panicking close must not mask the result.
    // SAFETY: `handle_c` is valid NUL-terminated UTF-8 for the call.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        (plugin.vt.stream_close)(handle_c.as_bytes_with_nul().as_ptr())
    }));
    let turn = turn.context("the plugin stream ended without a done event")?;
    let turn: AssistantTurn =
        serde_json::from_value(turn).with_context(|| "the done event's `turn` is malformed")?;
    Ok(turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::process::Command;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pi-rust-plugin-{}-{}-{}",
            tag,
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The good stub: name `stubprov`, one model, a scripted
    /// delta → done stream, and a login that reports done immediately.
    /// `__MODEL_ID__` is substituted per build (the override test).
    const GOOD_STUB: &str = r#"
use std::sync::atomic::{AtomicU32, Ordering};
#[repr(C)]
struct Vt {
    abi_version: extern "C" fn() -> u32,
    name: extern "C" fn() -> *const u8,
    list_models: extern "C" fn() -> *const u8,
    stream_open: extern "C" fn(*const u8) -> *const u8,
    stream_next: extern "C" fn(*const u8) -> *const u8,
    stream_close: extern "C" fn(*const u8),
    auth_start: extern "C" fn(*const u8) -> *const u8,
    auth_status: extern "C" fn(*const u8) -> *const u8,
    dispose: extern "C" fn(),
}
extern "C" fn abi_version() -> u32 { 1 }
static NAME: &[u8] = b"stubprov\0";
extern "C" fn name() -> *const u8 { NAME.as_ptr() }
static MODELS: &[u8] = b"[{\"id\":\"__MODEL_ID__\",\"name\":\"S\",\"api\":\"openai-completions\",\"provider\":\"stubprov\",\"input\":[\"text\"],\"contextWindow\":4096,\"maxTokens\":512}]\0";
extern "C" fn list_models() -> *const u8 { MODELS.as_ptr() }
static STEPS: [&[u8]; 3] = [
    b"{\"handle\":\"h1\"}\0" as &[u8],
    b"{\"type\":\"delta\",\"delta\":\"hi\"}\0" as &[u8],
    b"{\"type\":\"done\",\"turn\":{\"text\":\"hi\",\"tool_calls\":[],\"usage\":{\"input\":1,\"output\":1,\"cache_read\":0,\"cache_write\":0}}}\0" as &[u8],
];
static NEXT: AtomicU32 = AtomicU32::new(0);
extern "C" fn stream_open(_r: *const u8) -> *const u8 { STEPS[0].as_ptr() }
extern "C" fn stream_next(_h: *const u8) -> *const u8 {
    match NEXT.fetch_add(1, Ordering::SeqCst) {
        0 => STEPS[1].as_ptr(),
        1 => STEPS[2].as_ptr(),
        _ => std::ptr::null(),
    }
}
extern "C" fn stream_close(_h: *const u8) {}
static AUTH: &[u8] = b"{\"state\":\"done\",\"credential\":{\"type\":\"api_key\",\"key\":\"stub-key\"}}\0";
extern "C" fn auth_start(_r: *const u8) -> *const u8 { AUTH.as_ptr() }
extern "C" fn auth_status(_r: *const u8) -> *const u8 { AUTH.as_ptr() }
extern "C" fn dispose() {}
static VT: Vt = Vt { abi_version, name, list_models, stream_open, stream_next, stream_close, auth_start, auth_status, dispose };
#[no_mangle] pub extern "C" fn pi_provider_plugin_init() -> *const Vt { &VT }
"#;

    const WRONG_VERSION_STUB: &str = r#"
#[repr(C)]
struct Vt {
    abi_version: extern "C" fn() -> u32,
    name: extern "C" fn() -> *const u8,
    list_models: extern "C" fn() -> *const u8,
    stream_open: extern "C" fn(*const u8) -> *const u8,
    stream_next: extern "C" fn(*const u8) -> *const u8,
    stream_close: extern "C" fn(*const u8),
    auth_start: extern "C" fn(*const u8) -> *const u8,
    auth_status: extern "C" fn(*const u8) -> *const u8,
    dispose: extern "C" fn(),
}
extern "C" fn abi_version() -> u32 { 999 }
static NAME: &[u8] = b"wrongver\0";
extern "C" fn name() -> *const u8 { NAME.as_ptr() }
extern "C" fn list_models() -> *const u8 { b"[]\0".as_ptr() }
extern "C" fn stream_open(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn stream_next(_h: *const u8) -> *const u8 { std::ptr::null() }
extern "C" fn stream_close(_h: *const u8) {}
extern "C" fn auth_start(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn auth_status(_r: *const u8) -> *const u8 { b"{}\0".as_ptr() }
extern "C" fn dispose() {}
static VT: Vt = Vt { abi_version, name, list_models, stream_open, stream_next, stream_close, auth_start, auth_status, dispose };
#[no_mangle] pub extern "C" fn pi_provider_plugin_init() -> *const Vt { &VT }
"#;

    /// Compiles `src` to a cdylib named `<dir>/<stem>.<ext>` with rustc
    /// (the stubs are dependency-free; rustc ships with every toolchain).
    fn compile_stub(dir: &Path, stem: &str, src: &str) -> PathBuf {
        let ext = plugin_exts()[0];
        let out = dir.join(format!("{stem}.{ext}"));
        let src_path = dir.join(format!("{stem}.rs"));
        std::fs::write(&src_path, src).unwrap();
        let st = Command::new("rustc")
            .args(["--edition", "2021", "--crate-type", "cdylib", "-O", "-o"])
            .arg(&out)
            .arg(&src_path)
            .output()
            .expect("running rustc");
        assert!(
            st.status.success(),
            "rustc failed: {}",
            String::from_utf8_lossy(&st.stderr)
        );
        out
    }

    fn load(bundled: Option<&Path>, user: Option<&Path>) -> PluginSet {
        PluginSet::load_dirs(bundled, user)
    }

    #[test]
    fn missing_dirs_yield_an_empty_set() {
        let tmp = tmpdir("missing");
        let set = load(Some(&tmp.join("nope")), Some(&tmp.join("nada")));
        assert!(set.all_models().is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn garbage_file_is_skipped_and_good_plugins_still_load() {
        let tmp = tmpdir("garbage");
        let user = tmp.join("user");
        std::fs::create_dir_all(&user).unwrap();
        // A truncated platform image header — dlopen must fail, not crash.
        let ext = plugin_exts()[0];
        let bytes: &[u8] = if ext == "dll" {
            b"MZ"
        } else {
            b"\x7fELF\x02\x01\x01 garbage"
        };
        std::fs::write(user.join(format!("pi-garbage.{ext}")), bytes).unwrap();
        let _good = compile_stub(&user, "pi-good", &GOOD_STUB.replace("__MODEL_ID__", "s1"));
        let set = load(None, Some(&user));
        assert!(set.provider("stubprov").is_some(), "the good plugin loads");
        assert_eq!(set.all_models().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn wrong_abi_version_is_skipped() {
        let tmp = tmpdir("wrongver");
        let user = tmp.join("user");
        std::fs::create_dir_all(&user).unwrap();
        let _wrong = compile_stub(&user, "pi-wrong", WRONG_VERSION_STUB);
        let set = load(None, Some(&user));
        assert!(
            set.provider("wrongver").is_none(),
            "mismatched version skipped"
        );
        assert!(set.all_models().is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn user_dir_shadows_bundled_same_name() {
        let tmp = tmpdir("override");
        let bundled = tmp.join("bundled");
        let user = tmp.join("user");
        std::fs::create_dir_all(&bundled).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let _a = compile_stub(&bundled, "pi-a", &GOOD_STUB.replace("__MODEL_ID__", "s1"));
        let _b = compile_stub(&user, "pi-b", &GOOD_STUB.replace("__MODEL_ID__", "s2"));
        let set = load(Some(&bundled), Some(&user));
        let models = set.all_models();
        assert_eq!(models.len(), 1, "one provider, the user dir wins");
        assert_eq!(models[0].id, "s2");
        assert_eq!(models[0].provider, "stubprov");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn agent_dir_with_key(model_id: &str) -> PathBuf {
        let d = tmpdir("agent");
        std::fs::write(
            d.join("auth.json"),
            json!({"stubprov": {"type": "api_key", "key": "stored-key"}}).to_string(),
        )
        .unwrap();
        let _ = model_id;
        d
    }

    #[test]
    fn driver_for_unknown_model_is_typed() {
        let tmp = tmpdir("drvmodel");
        let user = tmp.join("user");
        std::fs::create_dir_all(&user).unwrap();
        let _s = compile_stub(&user, "pi-good", &GOOD_STUB.replace("__MODEL_ID__", "s1"));
        let set = load(None, Some(&user));
        let pl = set.provider("stubprov").unwrap().clone();
        let err = set
            .driver(&pl, Some("nope"), &tmpdir("a2"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not provided by provider plugin"), "{err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn driver_without_credential_is_typed() {
        let tmp = tmpdir("drvcred");
        let user = tmp.join("user");
        std::fs::create_dir_all(&user).unwrap();
        let _s = compile_stub(&user, "pi-good", &GOOD_STUB.replace("__MODEL_ID__", "s1"));
        let set = load(None, Some(&user));
        let pl = set.provider("stubprov").unwrap().clone();
        let err = set
            .driver(&pl, Some("s1"), &tmpdir("a3"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no credentials for provider stubprov"),
            "{err}"
        );
        assert!(err.contains("pi-rust auth login stubprov"), "{err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The full FFI round trip: the stub's scripted delta → done stream
    /// becomes an AssistantTurn, deltas forwarded in order.
    #[tokio::test]
    async fn stream_roundtrip_through_the_vtable() {
        let tmp = tmpdir("stream");
        let user = tmp.join("user");
        std::fs::create_dir_all(&user).unwrap();
        let _s = compile_stub(&user, "pi-good", &GOOD_STUB.replace("__MODEL_ID__", "s1"));
        let set = load(None, Some(&user));
        let pl = set.provider("stubprov").unwrap().clone();
        let drv = set
            .driver(&pl, Some("s1"), &agent_dir_with_key("s1"))
            .unwrap();
        let deltas: std::sync::Mutex<Vec<StreamDelta>> = Default::default();
        let d2 = &deltas;
        let turn = drv
            .complete(
                &[json!({"role": "user", "content": "hi"})],
                Box::new(move |d: StreamDelta| {
                    d2.lock().unwrap().push(d);
                }),
            )
            .await
            .expect("the stream completes");
        assert_eq!(turn.text.as_deref(), Some("hi"));
        let got = d2.lock().unwrap();
        assert_eq!(got.len(), 3, "TextStart + TextDelta + TextEnd: {got:?}");
        assert_eq!(turn.usage.input, 1);
        // The credential reached the plugin-side request (the stub ignores
        // it; the e2e asserts the wire header).
        drop(got);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn auth_status_shapes_parse() {
        assert!(matches!(
            parse_auth_status(&json!({"state":"pending","url":"http://x"}), "p"),
            Ok(AuthStatus::Pending { .. })
        ));
        match parse_auth_status(
            &json!({"state":"done","credential":{"type":"api_key"}}),
            "p",
        ) {
            Ok(AuthStatus::Done { credential }) => assert_eq!(credential["type"], "api_key"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_auth_status(&json!({"state":"failed","error":"nope"}), "p"),
            Ok(AuthStatus::Failed { .. })
        ));
        assert!(parse_auth_status(&json!({"state":"weird"}), "p").is_err());
        assert!(parse_auth_status(&json!({}), "p").is_err());
    }
}
