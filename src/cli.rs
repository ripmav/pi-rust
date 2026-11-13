use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent::{Core, EventLog, Item, QueuedUser};
use crate::client::{BridgeClient, Decision, INIT_TIMEOUT_MS};
use crate::model::FakeDriver;
use crate::provider::{build_provider, ProviderDriver};
use crate::session::Session;
use crate::transport;

struct RunSpec {
    text: String,
    steps: Vec<Value>,
    /// (mime, base64) attachments for `--image` (first prompt run only).
    images: Vec<(String, String)>,
    /// WP-704: the fake driver's compaction summary (scenario `compact_summary`).
    compact_summary: String,
}

/// MIME detection by magic bytes (png/jpeg/gif/webp — the formats the
/// upstream `@file` image path accepts).
fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
	if bytes.len() > 7 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
		Some("image/png")
	} else if bytes.len() > 2 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
		Some("image/jpeg")
	} else if bytes.len() > 3 && &bytes[..4] == b"GIF8" {
		Some("image/gif")
	} else if bytes.len() > 11 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
		Some("image/webp")
	} else {
		None
	}
}

fn b64(data: &[u8]) -> String {
	const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
	let mut out = String::new();
	for chunk in data.chunks(3) {
		let b0 = chunk[0];
		let b1 = *chunk.get(1).unwrap_or(&0);
		let b2 = *chunk.get(2).unwrap_or(&0);
		out.push(T[(b0 >> 2) as usize] as char);
		out.push(T[(((b0 & 3) << 4) | (b1 >> 4)) as usize] as char);
		out.push(if chunk.len() > 1 { T[(((b1 & 15) << 2) | (b2 >> 6)) as usize] as char } else { '=' });
		out.push(if chunk.len() > 2 { T[(b2 & 63) as usize] as char } else { '=' });
	}
	out
}

/// Read + validate an `--image` attachment: typed errors for missing files
/// and unsupported formats (S8).
fn read_image_attachment(path: &Path) -> Result<(String, String)> {
	let bytes = std::fs::read(path)
		.with_context(|| format!("--image: cannot read file: {}", path.display()))?;
	let mime = match detect_image_mime(&bytes) {
		Some(m) => m,
		None => bail!("--image: not a supported image format (png, jpeg, gif, webp): {}", path.display()),
	};
	Ok((mime.to_string(), b64(&bytes)))
}

/// Scenario JSON: `user_messages: [string]` plus per-message `model_scripts`
/// (index-aligned; omitted = empty script). Steps:
/// `{"text":"..."}` | `{"tool_call":{"name":"...","input":{...}}}`.
/// WP-704: optional top-level `compact: {window, reserve, keep}` (JSON
/// numbers; the fake model has no catalog contextWindow, so the override
/// drives the auto trigger) and `compact_summary` (the fake's summary text).
/// Returns (runs, the compact override).
fn parse_scenario(v: &Value) -> Result<(Vec<RunSpec>, Option<(u64, u64, u64)>)> {
    let user_messages = v.get("user_messages").and_then(Value::as_array).context("scenario needs `user_messages: [string]`")?;
    let scripts = v.get("model_scripts").and_then(Value::as_array).cloned().unwrap_or_default();
    let compact = match v.get("compact") {
        None => None,
        Some(c) => {
            let field = |k: &str| -> Result<u64> {
                c.get(k).and_then(Value::as_u64).with_context(|| format!("scenario compact.{k} must be a JSON number"))
            };
            Some((field("window")?, field("reserve")?, field("keep")?))
        }
    };
    let compact_summary = v
        .get("compact_summary")
        .and_then(Value::as_str)
        .unwrap_or(crate::model::DEFAULT_COMPACT_SUMMARY)
        .to_string();
    let mut runs = Vec::new();
    for (i, m) in user_messages.iter().enumerate() {
        let text = m.as_str().with_context(|| format!("user_messages[{i}] must be a string"))?.to_string();
        let steps = scripts.get(i).and_then(Value::as_array).cloned().unwrap_or_default();
        runs.push(RunSpec { text, steps, images: Vec::new(), compact_summary: compact_summary.clone() });
    }
    if runs.is_empty() { bail!("scenario needs at least one user message"); }
    Ok((runs, compact))
}

fn find_sidecar(cwd: &Path) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("PI_RUST_SIDECAR") {
        return Ok(PathBuf::from(p));
    }
    // Walk up from cwd, then from the executable's directory (a deployed
    // binary finds its own sidecar regardless of cwd).
    let mut roots = vec![cwd.to_path_buf()];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            roots.push(d.to_path_buf());
        }
    }
    for root in roots {
        let mut dir = root;
        for _ in 0..6 {
            let cand = dir.join("sidecar").join("index.mjs");
            if cand.is_file() {
                return Ok(cand);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    bail!("sidecar/index.mjs not found from {} (set PI_RUST_SIDECAR)", cwd.display())
}

/// Non-interactive scenario runner (no REPL in v0).
pub async fn run() -> Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut ext_dirs: Vec<String> = Vec::new();
    let mut agent_dir: Option<PathBuf> = None;
    let mut isolated = false;
    let mut scenario_path: Option<PathBuf> = None;
    let mut session_file: Option<PathBuf> = None;
    let mut list_sessions = false;
    let mut print_usage = false;
    let mut resume_ref: Option<String> = None;
    let mut fork_ref: Option<String> = None;
    let mut commands: Vec<(String, Vec<String>)> = Vec::new();
    let mut provider_flag: Option<String> = None;
    let mut model_id: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut image_paths: Vec<PathBuf> = Vec::new();
    let mut out_mode: Option<crate::agent::OutputMode> = None;
    let mut template_name: Option<String> = None;
    let mut template_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let val = |args: &Vec<String>, i: &mut usize| -> Result<String> {
            *i += 1;
            args.get(*i).cloned().context("missing value for argument")
        };
        match args[i].as_str() {
            "--extension-dir" => ext_dirs.push(val(&args, &mut i)?),
            "--agent-dir" => agent_dir = Some(PathBuf::from(val(&args, &mut i)?)),
            "--isolated" => isolated = true,
            "--scenario" => scenario_path = Some(PathBuf::from(val(&args, &mut i)?)),
            "--session-file" => session_file = Some(PathBuf::from(val(&args, &mut i)?)),
            "--list-sessions" => list_sessions = true,
            "--print-usage" => print_usage = true,
            "--resume" => resume_ref = Some(val(&args, &mut i)?),
            "--fork" => fork_ref = Some(val(&args, &mut i)?),
            "--provider" => provider_flag = Some(val(&args, &mut i)?),
            "--model" => model_id = Some(val(&args, &mut i)?),
            "--prompt" => prompt = Some(val(&args, &mut i)?),
            // WP-610: one-shot stdout variants (upstream print-mode parity).
            "-p" | "--print" => {
                let m = out_mode.unwrap_or(crate::agent::OutputMode::Text);
                anyhow::ensure!(m == crate::agent::OutputMode::Text, "--mode json conflicts with --print (text)");
                out_mode = Some(crate::agent::OutputMode::Text);
            }
            "--mode" => {
                let v = val(&args, &mut i)?;
                let m = match v.as_str() {
                    "text" => crate::agent::OutputMode::Text,
                    "json" => crate::agent::OutputMode::Json,
                    other => bail!("invalid --mode '{other}' (valid: text, json)"),
                };
                anyhow::ensure!(out_mode.map(|o| o == m).unwrap_or(true), "--mode conflicts with --print (text)");
                out_mode = Some(m);
            }
            "--image" => image_paths.push(PathBuf::from(val(&args, &mut i)?)),
            "--template" => {
                template_name = Some(val(&args, &mut i)?);
                // Template args: positional tokens until the next flag.
                while i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    i += 1;
                    template_args.push(args[i].clone());
                }
            }
            "--command" => {
                let name = val(&args, &mut i)?;
                // Variadic args: consume until the next `--` flag (or end).
                let mut cargs: Vec<String> = Vec::new();
                while i + 1 < args.len() && !args[i + 1].starts_with("--") {
                    i += 1;
                    cargs.push(args[i].clone());
                }
                commands.push((name, cargs));
            }
            other => bail!("unknown argument: {other}\nusage: pi-rust --extension-dir <p>... [--agent-dir <p>] [--command <name> [args...]]... --scenario <s> | --prompt <text> [-p|--print] [--mode text|json] [--session-file <p>] [--resume <file|id>] [--fork <file|id>] [--list-sessions] [--provider <id>] [--model <id>]"),
        }
        i += 1;
    }
    if let Some(m) = out_mode {
        use crate::agent::OutputMode::*;
        anyhow::ensure!(
            prompt.is_some() || scenario_path.is_some(),
            "{:?} mode requires --prompt <text> or --scenario <file>",
            if m == Json { "json" } else { "print" }
        );
        anyhow::ensure!(!list_sessions, "print/json mode cannot be combined with --list-sessions");
    }

    let cwd = std::env::current_dir().context("cwd")?;

    // Agent dir (S6): --agent-dir > PI_CODING_AGENT_DIR > ~/.pi/agent
    // (upstream getAgentDir). --isolated forces a fresh temp dir (the M1
    // hermetic behavior, now opt-in).
    let agent_dir = if isolated {
        std::env::temp_dir().join(format!("pi-rust-agent-{}", uuid::Uuid::new_v4()))
    } else {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
        crate::config::resolve_agent_dir(agent_dir.as_deref(), std::env::var("PI_CODING_AGENT_DIR").ok().as_deref(), &home)
    };
    std::fs::create_dir_all(&agent_dir).with_context(|| format!("creating agent dir {}", agent_dir.display()))?;

    // --scenario/--prompt/--template are mutually exclusive (original flags).
    if scenario_path.is_some() && (prompt.is_some() || template_name.is_some())
        || prompt.is_some() && template_name.is_some()
    {
        bail!("--scenario, --prompt and --template are mutually exclusive");
    }
    if !image_paths.is_empty() && scenario_path.is_some() {
        bail!("--image requires --prompt or --template (v0: no scenario attachments)");
    }
    // --template expands to the prompt (S7); discovery reads
    // <agentDir>/prompts + .pi/prompts.
    if let Some(name) = &template_name {
        let text = crate::templates::apply(&agent_dir, &cwd, name, &template_args.join(" "))?;
        prompt = Some(text);
    }

    let (runs, compact_override) = match (&scenario_path, &prompt) {
        (Some(p), None) => {
            let raw = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            parse_scenario(&serde_json::from_str(&raw).context("scenario must be a JSON object")?)?
        }
        (Some(_), Some(_)) => unreachable!("checked above: scenario + prompt/template are exclusive"),
        (None, Some(text)) => (vec![RunSpec {
            text: text.clone(),
            steps: vec![],
            images: image_paths.iter().map(|p| read_image_attachment(p)).collect::<Result<_>>()?,
            compact_summary: crate::model::DEFAULT_COMPACT_SUMMARY.to_string(),
        }], None),
        (None, None) => {
            if list_sessions {
                (Vec::new(), None) // --list-sessions needs no scenario
            } else if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                (Vec::new(), None) // interactive TUI (WP-601)
            } else {
                bail!("--scenario <file>, --prompt <text> or --template <name> is required (or --list-sessions)")
            }
        }
    };

    // Isolated agent dir (owned by the core when defaulted) so extension
    // discovery stays deterministic. Upstream semantics: ctx.cwd is the
    // project directory (the child inherits the core's cwd); the agent dir
    // is discovery-only.
    // M3/WP-301: agent settings from the agent dir (global file only — the
    // project `cwd/.pi/settings.json` merge needs a trust model, out of
    // scope for the headless core). Flags beat settings; settings beat the
    // fake-driver default.
    if list_sessions {
        list_sessions_json(&agent_dir)?;
        return Ok(0);
    }

    let agent_config = crate::config::load(&agent_dir).with_context(|| "loading agent config")?;
    // WP-702/702c: agent-dir MCP server config (absent file = no servers).
    let mcp_config = crate::config::load_mcp_config(&agent_dir).with_context(|| "loading agent mcp config")?;
    let (provider_name, model_id) =
        crate::config::resolve_provider_flags(provider_flag.as_deref(), model_id.as_deref(), &agent_config)?;
    // S3: provider credentials come from env only; the fake driver needs none.
    // WP-701: a name unknown to the bundled registry may be an
    // extension-registered provider — no driver is built at startup; the
    // bridge routes it after `ready` (typed error if never registered).
    let builtin = crate::provider::is_builtin_provider(&provider_name);
    if !builtin && model_id.is_none() {
        anyhow::bail!("--provider {provider_name} (not a built-in provider) requires --model <id>");
    }
    let provider: Option<ProviderDriver> = if builtin {
        build_provider(&provider_name, model_id.as_deref(), &agent_dir)?
    } else {
        None
    };
    let fake_mode = builtin && provider.is_none();
    let initial_model = if fake_mode {
        json!({"id":"fake","provider":"fake"})
    } else {
        json!({"id":model_id,"provider":provider_name})
    };
    let model_label = format!(
        "{}/{}",
        initial_model.get("provider").and_then(Value::as_str).unwrap_or("?"),
        initial_model.get("id").and_then(Value::as_str).unwrap_or("?")
    );

    // S8: --image requires an image-capable registry model (openai/anthropic
    // drivers emit the wire parts; other drivers drop images).
    if !image_paths.is_empty() {
        if fake_mode {
            anyhow::bail!("--image requires a provider model (the fake driver has no image support)");
        }
        let model_str = model_id.as_deref().unwrap_or_default();
        let m = crate::registry::ModelRegistry::bundled()
            .resolve(&provider_name, model_str)
            .ok_or_else(|| anyhow::anyhow!("--image: model {provider_name}/{model_str} not in the bundled registry"))?;
        if !m.input.iter().any(|i| i == "image") {
            anyhow::bail!("--image: model {provider_name}/{model_str} does not accept image input (input: {})", m.input.join(", "));
        }
        if provider_name != "openai" && provider_name != "anthropic" {
            // OpenAI-compatible providers carry the OpenAI wire; anthropic is
            // native. Other v0 drivers do not emit image parts.
            let is_openai_wire = provider_name == "openai"
                || crate::provider::compat_family_base(&provider_name).is_some()
                || provider_name == "azure"
                || provider_name.starts_with("cloudflare");
            if !is_openai_wire {
                anyhow::bail!("--image: provider {provider_name} does not support image input in v0");
            }
        }
    }

    // Session file: --resume/--fork continue an existing session;
    // --session-file points at one; else the upstream layout under the
    // agent dir (`<agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl`).
    if resume_ref.is_some() && fork_ref.is_some() {
        anyhow::bail!("--resume and --fork are mutually exclusive");
    }
    let (session_file, existing, fork_from) = match (&resume_ref, &fork_ref) {
        (Some(r), _) => (resolve_session_ref(&agent_dir, r)?, true, None),
        (None, Some(r)) => {
            let old = resolve_session_ref(&agent_dir, r)?;
            let dir = old.parent().context("forked session file has no parent dir")?;
            let ts = crate::session::now_iso().replace(":", "-");
            (dir.join(format!("{}_{}.jsonl", ts, uuid::Uuid::new_v4())), false, Some(old))
        }
        _ => {
            let sf = session_file.unwrap_or_else(|| {
                let dir = agent_dir.join("sessions").join(crate::session::encode_cwd(&cwd.to_string_lossy()));
                std::fs::create_dir_all(&dir).ok();
                let ts = crate::session::now_iso().replace(":", "-");
                dir.join(format!("{}_{}.jsonl", ts, uuid::Uuid::new_v4()))
            });
            let exists = sf.exists();
            (sf, exists, None)
        },
    };
    let session = Arc::new(match fork_from {
        Some(old) => Session::fork(&old, &session_file).context("forking session")?,
        _ if existing => Session::load(&session_file).context("resuming session")?,
        _ => Session::create(session_file.clone(), &cwd.to_string_lossy()).context("creating session file")?,
    });

    let sidecar = find_sidecar(&cwd)?;
    let node = std::env::var("PI_RUST_NODE").unwrap_or_else(|_| "node".into());
    let mut sidecar_args: Vec<String> = Vec::new();
    for d in &ext_dirs {
        // Absolute paths keep the sidecar's extension discovery cwd-independent.
        let p = PathBuf::from(d);
        let abs = if p.is_absolute() { p } else { cwd.join(&p) };
        sidecar_args.push("--extension-dir".into());
        sidecar_args.push(abs.to_string_lossy().into());
    }
    sidecar_args.push("--agent-dir".into());
    sidecar_args.push(agent_dir.to_string_lossy().into());

    // WP-601: interactive mode — no --scenario/--prompt and a TTY on stdin.
    let tui_mode = scenario_path.is_none() && prompt.is_none() && !list_sessions;

    // TUI mode owns the terminal: the sidecar's stderr and the core's own
    // diagnostic lines must not corrupt the screen — both go to a log file
    // in the agent dir.
    // shortcut: the sink-setup error line itself stays on stderr (rare, pre-TUI).
    let sidecar_stderr = if tui_mode {
        let tui_log = agent_dir.join("tui-stderr.log");
        match std::fs::File::create(&tui_log) {
            Ok(f) => {
                crate::corelog::set_log_file(&tui_log);
                Stdio::from(f)
            }
            Err(e) => {
                eprintln!("[core] tui stderr log: {e}");
                Stdio::inherit()
            }
        }
    } else {
        Stdio::inherit() // sidecar logs pass through to the core's stderr
    };
    crate::corelog::line(&format!("[core] spawning sidecar: {node} {}", sidecar.display()));

    // WP-610: the NDJSON-on-stdout contract applies to the default headless
    // mode only; --print/--mode json keep stdout clean (their own streams).
    let log = EventLog::start(!tui_mode && out_mode != Some(crate::agent::OutputMode::Text) && out_mode != Some(crate::agent::OutputMode::Json));
    let fail: Arc<Mutex<Option<String>>> = Arc::default();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    // WP-604: blocking ui dialog requests for the TUI (headless: unused,
    // the default-answers path handles them in the event task).
    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    // WP-702: drain barrier between the agent loop and the event task
    // (tools_changed must be applied before the tool set is read).
    let (tools_sync_tx, tools_sync_rx) = tokio::sync::mpsc::unbounded_channel();

    // Child inherits the core's cwd (upstream: ctx.cwd = project dir).
    let (child, tx, rx) = transport::spawn(&node, &sidecar, &sidecar_args, &cwd, sidecar_stderr).await?;
    let (client, events_rx) = BridgeClient::connect(child, tx, rx);
    let client = Arc::new(client);

    // Handshake: init → success response → ready event → state Running.
    // WP-505: a headless core advertises no UI methods — the sidecar then
    // keeps the upstream noOp UI context (hasUI=false) and extensions take
    // their no-UI defaults (upstream print-mode behavior). The TUI client
    // (M6/WP-604) advertises its methods to get the dialog wiring.
    let ui_methods = if tui_mode {
        json!(["select", "confirm", "input", "editor", "notify"])
    } else {
        json!([])
    };
    // WP-705: the resolved keybinding table (upstream schema + user file)
    // goes to the sidecar for the extension-shortcut precedence resolution
    // and to the TUI for dispatch; an invalid file falls back to defaults
    // (the TUI then shows the error in the status bar).
    let (kb, kb_error) = match crate::tui::keybindings::KeybindingManager::load(&agent_dir) {
        Ok(kb) => (kb, None),
        Err(e) => {
            crate::corelog::line(&format!("[core] keybindings: {e}"));
            (crate::tui::keybindings::KeybindingManager::default(), Some(e))
        }
    };
    let mut init = json!({"protocolVersion":4,"capabilities":{"ui":ui_methods,"timeoutMs":INIT_TIMEOUT_MS}});
    // WP-702/702c: pass the agent-dir MCP config through (absent = omitted).
    if let Some(cfg) = &mcp_config {
        init["mcpConfig"] = cfg.clone();
    }
    // WP-705: resolved action -> keys (the sidecar runs upstream
    // getShortcuts against this table).
    let mut kb_map = serde_json::Map::new();
    for a in crate::tui::keybindings::valid_actions() {
        kb_map.insert(a.clone(), json!(kb.get_keys(&a)));
    }
    init["keybindings"] = Value::Object(kb_map);
    match client.request_init("init", init).await {
        Decision::Response(r) if r.get("success") == Some(&json!(true)) => {}
        Decision::Response(r) => bail!("init failed: {}", r.get("error").and_then(Value::as_str).unwrap_or("?")),
        Decision::Unavailable => bail!("init unanswered: sidecar transport lost"),
    }

    let ctx = Arc::new(Core {
        client: client.clone(),
        session: session.clone(),
        events: Arc::default(),
        ext_tools: Arc::default(),
        ext_models: Arc::default(),
        ready: Mutex::new(Some(ready_tx)),
        log: log.clone(),
        fail: fail.clone(),
        model: Arc::new(Mutex::new((initial_model, agent_config.default_thinking_level.clone()))),
        msg_tx: msg_tx.clone(),
        done: Mutex::new(Some(done_tx)),
        provider: Arc::new(Mutex::new(provider)),
        agent_dir: Arc::new(agent_dir.clone()),
        usage_total: Arc::default(),
        cost_total: Arc::default(),
        tui: tui_mode,
        ui_tx,
        tools_sync_tx,
        compact_override,
        out_mode: out_mode.unwrap_or(crate::agent::OutputMode::Ndjson),
        text_final: Arc::default(),
        json_tx: {
            if out_mode == Some(crate::agent::OutputMode::Json) {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let mut out = tokio::io::BufWriter::new(tokio::io::stdout());
                    while let Some(line) = rx.recv().await {
                        let _ = out.write_all(format!("{}\n", line.trim_end()).as_bytes()).await;
                        let _ = out.flush().await;
                    }
                });
                Some(tx)
            } else {
                None
            }
        },
    });
    // WP-610: the json stream opens with the session header (upstream parity).
    if ctx.out_mode == crate::agent::OutputMode::Json {
        if let Some(h) = session.header() {
            ctx.json_tx.as_ref().unwrap().send(h.to_string()).ok();
        }
    }
    tokio::spawn(crate::agent::event_task(events_rx, tools_sync_rx, ctx.clone()));
    tokio::spawn(crate::agent::agent_task(ctx.clone(), msg_rx));

    let ready = tokio::time::timeout(Duration::from_millis(INIT_TIMEOUT_MS), ready_rx)
        .await.context("sidecar did not send `ready` in time")?.context("sidecar event task exited before `ready`")?;
    if ready.get("apiVersion").and_then(Value::as_u64) != Some(0) {
        bail!("sidecar apiVersion != 0");
    }
    // WP-701: a startup --provider unknown to the bundled registry must have
    // been registered by an extension — typed error before the run starts
    // (no silent fake-driver fallback).
    if !builtin {
        let ext = crate::agent::parse_providers(&ready);
        let key = format!("{provider_name}/{}", model_id.as_deref().unwrap_or(""));
        if !ext.contains_key(&key) {
            bail!("provider {key} is not a built-in provider and was not registered by an extension (check --extension-dir)");
        }
    }
    for e in ready.get("errors").and_then(Value::as_array).into_iter().flatten() {
        crate::corelog::line(&format!("[core] extension load error: {e}"));
    }
    client.set_running();
    {
        let ev = ctx.events.lock().unwrap().clone();
        crate::corelog::line(&format!("[core] running: subscribed events {ev:?}"));
        if ev.contains("session_start") {
            client.fire(json!({"type":"session_start","event":{"type":"session_start","reason":"startup"}}));
        }
    }

    // WP-705: extension shortcuts (the sidecar applied the upstream
    // precedence resolution) + its conflict diagnostics, which the core
    // surfaces on stderr like upstream prints them.
    let mut ext_shortcuts: Vec<(String, String)> = Vec::new();
    for sc in ready.get("shortcuts").and_then(Value::as_array).into_iter().flatten() {
        if let Some(key) = sc.get("key").and_then(Value::as_str) {
            ext_shortcuts.push((
                key.to_string(),
                sc.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
            ));
        }
    }
    for d in ready.get("shortcutDiagnostics").and_then(Value::as_array).into_iter().flatten() {
        if let Some(m) = d.as_str() {
            crate::corelog::line(&format!("[core] {m}"));
        }
    }

    // --command: run each registered extension command after ready, before the
    // scenario. Unknown command or handler error logs ok:false + error and
    // continues (does not abort the run). TUI mode runs them on a task after
    // the TUI loop is up: a command that opens a dialog would otherwise
    // block its own response (the dialog is answered in the TUI).
    if tui_mode {
        let c_client = client.clone();
        let c_log = log.clone();
        let c_commands = commands.clone();
        tokio::spawn(async move {
            for (name, cargs) in &c_commands {
                let resp = c_client.request("command", json!({"name":name,"args":cargs})).await;
                let (ok, error) = match resp {
                    Decision::Response(r) if r.get("success") == Some(&json!(true)) => (true, None),
                    Decision::Response(r) => (false, r.get("error").and_then(Value::as_str).map(str::to_string)),
                    Decision::Unavailable => (false, Some("command unanswered: bridge not running".into())),
                };
                let mut line = json!({"kind":"command","name":name,"ok":ok});
                if let Some(e) = error {
                    line["error"] = json!(e);
                }
                c_log.emit(line);
            }
        });
    } else {
        for (name, cargs) in &commands {
        let resp = client.request("command", json!({"name":name,"args":cargs})).await;
        let (ok, error) = match resp {
            Decision::Response(r) if r.get("success") == Some(&json!(true)) => (true, None),
            Decision::Response(r) => (false, r.get("error").and_then(Value::as_str).map(str::to_string)),
            Decision::Unavailable => (false, Some("command unanswered: bridge not running".into())),
        };
        let mut line = json!({"kind":"command","name":name,"ok":ok});
        if let Some(e) = error {
            line["error"] = json!(e);
        }
        log.emit(line);
        }
    }

    // Interactive mode: the TUI owns input and lifetime; the agent loop runs
    // until the user quits (Item::End). Non-interactive: seed the scripted
    // runs and end immediately.
    if tui_mode {
        // WP-606: upstream keybinding schema + <agentDir>/keybindings.json.
        // An invalid file is logged (with the valid actions) and the TUI
        // starts on the defaults with the error in the status bar.
        // WP-608: the slash menu gets the extension commands (ready
        // payload) and the discovered prompt templates (WP-307).
        let mut ext_cmds: Vec<crate::tui::commands::Command> = Vec::new();
        if let Some(arr) = ready.get("commands").and_then(serde_json::Value::as_array) {
            for c in arr {
                ext_cmds.push(crate::tui::commands::Command {
                    name: c.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                    description: c.get("description").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
                    argument_hint: None,
                    source: crate::tui::commands::Source::Extension,
                });
            }
        }
        let tui_templates = crate::templates::discover(&agent_dir, &cwd);
        let mut app = crate::tui::TuiApp::with_kb(
            kb,
            model_label,
            cwd.display().to_string(),
            session.id.lock().unwrap().clone(),
        )
        .with_shortcuts(ext_shortcuts)
        .with_commands(ext_cmds, tui_templates);
        if let Some(e) = kb_error {
            app.last_error = Some(format!("keybindings: {e}"));
        }
        // WP-607: the `theme` setting selects the starting theme (a
        // `light/dark` auto pair takes its dark side in v0). An invalid name
        // keeps the built-in dark theme and reports in the status bar.
        if let Some(setting) = agent_config.theme.as_deref() {
            if let Some(name) = crate::tui::theme::resolve_theme_setting(Some(setting)) {
                match crate::tui::theme::Theme::load(&agent_dir, &name) {
                    Ok(t) => {
                        app.theme = t;
                    }
                    Err(e) => {
                        crate::corelog::line(&format!("[core] theme: {e}"));
                        app.last_error = Some(format!("theme: {e}"));
                    }
                }
            }
        }
        let submit_tx = msg_tx.clone();
        let end_tx = msg_tx.clone();
        let dialog_client = client.clone();
        let dialog_log = log.clone();
        let result = crate::tui::terminal::run(
            &mut app,
            log.subscribe(),
            ui_rx,
            move |text| {
                let _ = submit_tx.send(Item::User(QueuedUser {
                    text: text.to_string(),
                    source: "tui",
                    steps: vec![],
                    images: vec![],
                    compact_summary: crate::model::DEFAULT_COMPACT_SUMMARY.to_string(),
                }));
            },
            move |outcome| match outcome {
                // WP-604: the answer flows back over the unchanged bridge
                // protocol; the event log records the round trip.
                crate::tui::dialog::DialogOutcome::Answered { id, method, title, message, response } => {
                    dialog_client.send(response.clone());
                    dialog_log.emit(json!({"kind":"extension_ui_request","id":id,"method":method,
                        "title":title,"message":message,"response":response}));
                }
                // Timeout: the sidecar already resolved with its upstream
                // default — the TUI only stops showing the dialog.
                crate::tui::dialog::DialogOutcome::TimedOut { id, method, title, message } => {
                    dialog_log.emit(json!({"kind":"extension_ui_request","id":id,"method":method,
                        "title":title,"message":message,"response":Value::Null,"timedOut":true}));
                }
            },
            agent_dir.clone(),
            {
                let req_client = client.clone();
                let req_log = log.clone();
                move |label, extra, ty, payload| {
                    // WP-608: the slash command's bridge round-trip; the
                    // result comes back as a TUI-only event rendered in the
                    // transcript.
                    let ty = ty.to_string();
                    let c = req_client.clone();
                    let l = req_log.clone();
                    tokio::spawn(async move {
                        let resp = c.request(&ty, payload).await;
                        let (ok, text) = match resp {
                            Decision::Response(r) if r.get("success") == Some(&json!(true)) => {
                                let data = r.get("data").cloned().unwrap_or(Value::Null);
                                let text = if data == json!({ "ok": true }) {
                                    String::new()
                                } else {
                                    data.as_str().map(str::to_string).unwrap_or_else(|| data.to_string())
                                };
                                (true, text)
                            }
                            Decision::Response(r) => (
                                false,
                                r.get("error").and_then(Value::as_str).unwrap_or("failed").to_string(),
                            ),
                            Decision::Unavailable => (false, "bridge not running".into()),
                        };
                        let mut ev = json!({
                            "kind": "bridge_cmd_result",
                            "label": label,
                            "ok": ok,
                            "text": text,
                        });
                        if ok {
                            if let Some(obj) = extra.as_object() {
                                for (k, v) in obj {
                                    ev[k] = v.clone();
                                }
                            }
                        }
                        l.emit_tui(ev);
                    });
                }
            },
            {
                let resume_session = session.clone();
                move |path: &str| -> Result<(String, Vec<serde_json::Value>), String> {
                    // WP-609: resume the selected file in place (the core
                    // keeps the same Arc<Session>; the identity/file
                    // swap is inside Session::resume_from) and snapshot
                    // id + transcript for the TUI display.
                    resume_session
                        .resume_from(std::path::Path::new(path))
                        .map_err(|e| e.to_string())?;
                    let id = resume_session.id.lock().unwrap().clone();
                    let msgs = resume_session
                        .transcript()
                        .into_iter()
                        .filter(|m| matches!(m.get("role").and_then(serde_json::Value::as_str), Some("user") | Some("assistant")))
                        .collect();
                    Ok((id, msgs))
                }
            },
            move || {
                let _ = end_tx.send(Item::End);
            },
        )
        .await;
        if let Err(e) = result {
            crate::corelog::line(&format!("[core] tui: {e}"));
        }
    } else {
        // Seed the runs: one agent run per user message. Fake mode validates each
        // model_script up front; provider mode ignores scripts (real completions).
        for r in &runs {
            if fake_mode {
                FakeDriver::from_steps(&r.steps, &r.compact_summary).with_context(|| "parsing model_script")?;
            }
            let source: &'static str = if r.steps.is_empty() && scenario_path.is_none() { "cli" } else { "rpc" };
            msg_tx.send(Item::User(QueuedUser { text: r.text.clone(), source, steps: r.steps.clone(), images: r.images.clone(), compact_summary: r.compact_summary.clone() })).unwrap();
        }
        msg_tx.send(Item::End).unwrap();
    }

    // Wait for the scenario (and any extension-injected user messages).
    let _ = done_rx.await;

    // Clean shutdown: session_shutdown → sidecar exits 0. The core marks the
    // state Stopped once the shutdown is sent (avoids misclassifying the
    // normal-exit race); a non-zero sidecar exit is still recorded as a
    // failure. A degraded bridge has no live sidecar to shut down: skip the
    // handshake and exit 3 below.
    if !client.degraded_seen() {
        client.fire(json!({"type":"session_shutdown","event":{"type":"session_shutdown","reason":"quit"}}));
        client.set_stopped();
        match tokio::time::timeout(Duration::from_secs(10), client.wait_exited()).await {
            Ok(Some(0)) => {}
            Ok(Some(c)) => {
                fail.lock().unwrap().get_or_insert(format!("sidecar exited with {c} during shutdown"));
            }
            Ok(None) => unreachable!("wait_exited resolves only on exit"),
            Err(_) => {
                crate::corelog::line("[core] sidecar did not exit 10s after session_shutdown");
                fail.lock().unwrap().get_or_insert("sidecar did not exit after session_shutdown".into());
            }
        }
    } else {
        client.set_stopped();
    }

    // Final event: the full session (acked: written+flushed before exit).
    // WP-610: print/json keep stdout clean — no usage/transcript events.
    if ctx.out_mode == crate::agent::OutputMode::Ndjson {
        if print_usage {
            let u = ctx.usage_total.lock().unwrap().clone();
            let cost = *ctx.cost_total.lock().unwrap();
            log.emit(json!({"kind":"usage","input":u.input,"output":u.output,
                "cacheRead":u.cache_read,"cacheWrite":u.cache_write,"costUsd":cost}));
        }
        log.emit_final(json!({"kind":"transcript","session":session.dump()})).await;
    } else if ctx.out_mode == crate::agent::OutputMode::Text {
        // Upstream print mode: the final assistant text, one trailing newline.
        if let Some(t) = ctx.text_final.lock().unwrap().take() {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{t}");
            let _ = out.flush();
        }
    }

    // 0 = clean; 2 = scenario/model-script or shutdown failure; 3 = degraded
    // (sidecar crashed mid-run — protocol: degraded is terminal, no respawn).
    // WP-610: print/json use the upstream exit codes (1 for any failure).
    Ok(match ctx.out_mode {
        crate::agent::OutputMode::Ndjson => {
            if client.degraded_seen() { 3 } else if fail.lock().unwrap().is_some() { 2 } else { 0 }
        }
        _ => {
            if client.degraded_seen() || fail.lock().unwrap().is_some() { 1 } else { 0 }
        }
    })
}

/// `--list-sessions`: one NDJSON line per session file under
/// `<agentDir>/sessions/` (recursive): {file, id, cwd, name, updated, entries}.
fn list_sessions_json(agent_dir: &Path) -> Result<()> {
    // WP-609: the shared session listing (same source as the TUI picker).
    for s in crate::session::list(agent_dir) {
        println!("{}", json!({
            "file": s.path.to_string_lossy(),
            "id": s.id,
            "cwd": s.cwd,
            "name": s.name,
            "parentSession": s.parent_session,
            "updated": s.updated,
            "entries": s.entries,
        }));
    }
    Ok(())
}

/// Resolves a `--resume`/`--fork` reference: an existing file path wins;
/// otherwise it is matched against session ids under the agent dir.
fn resolve_session_ref(agent_dir: &Path, ref_: &str) -> Result<PathBuf> {
    let p = PathBuf::from(ref_);
    if p.exists() {
        return Ok(p);
    }
    for f in crate::session::list(agent_dir).iter().map(|s| s.path.clone()) {
        let Ok(raw) = std::fs::read_to_string(&f) else { continue };
        let Some(first) = raw.lines().next() else { continue };
        if let Ok(h) = serde_json::from_str::<Value>(first) {
            if h.get("type").and_then(Value::as_str) == Some("session")
                && h.get("id").and_then(Value::as_str) == Some(ref_)
            {
                return Ok(f.clone());
            }
        }
    }
    anyhow::bail!("no session {ref_}: not an existing file and no session id match under {}", agent_dir.join("sessions").display())
}

#[cfg(test)]
mod image_tests {
	use super::*;

	const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
	const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2];
	const GIF: &[u8] = b"GIF89a1234";
	const WEBP: &[u8] = b"RIFF\x0c\x00\x00\x00WEBPVP8 ";

	#[test]
	fn detects_supported_mimes() {
		assert_eq!(detect_image_mime(PNG), Some("image/png"));
		assert_eq!(detect_image_mime(JPEG), Some("image/jpeg"));
		assert_eq!(detect_image_mime(GIF), Some("image/gif"));
		assert_eq!(detect_image_mime(WEBP), Some("image/webp"));
		assert_eq!(detect_image_mime(b"plain text"), None);
		assert_eq!(detect_image_mime(b"RIFFxxxxxxx"), None); // RIFF but not WEBP
	}

	#[test]
	fn b64_encodes_std_vectors() {
		assert_eq!(b64(b""), "");
		assert_eq!(b64(b"f"), "Zg==");
		assert_eq!(b64(b"fo"), "Zm8=");
		assert_eq!(b64(b"foo"), "Zm9v");
		assert_eq!(b64(b"foob"), "Zm9vYg==");
	}

	#[test]
	fn read_image_attachment_typed_errors() {
		let dir = std::env::temp_dir().join(format!("pi-rust-img-{}", uuid::Uuid::new_v4()));
		std::fs::create_dir_all(&dir).unwrap();
		let png = dir.join("a.png");
		std::fs::write(&png, PNG).unwrap();
		let (mime, data) = read_image_attachment(&png).unwrap();
		assert_eq!(mime, "image/png");
		assert_eq!(data, b64(PNG));
		let missing = dir.join("nope.png");
		let err = read_image_attachment(&missing).unwrap_err().to_string();
		assert!(err.contains("cannot read file"), "{err}");
		let txt = dir.join("b.png");
		std::fs::write(&txt, b"definitely not an image").unwrap();
		let err = read_image_attachment(&txt).unwrap_err().to_string();
		assert!(err.contains("not a supported image format"), "{err}");
	}
}
