use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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
}

/// Scenario JSON: `user_messages: [string]` plus per-message `model_scripts`
/// (index-aligned; omitted = empty script). Steps:
/// `{"text":"..."}` | `{"tool_call":{"name":"...","input":{...}}}`.
fn parse_scenario(v: &Value) -> Result<Vec<RunSpec>> {
    let user_messages = v.get("user_messages").and_then(Value::as_array).context("scenario needs `user_messages: [string]`")?;
    let scripts = v.get("model_scripts").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut runs = Vec::new();
    for (i, m) in user_messages.iter().enumerate() {
        let text = m.as_str().with_context(|| format!("user_messages[{i}] must be a string"))?.to_string();
        let steps = scripts.get(i).and_then(Value::as_array).cloned().unwrap_or_default();
        runs.push(RunSpec { text, steps });
    }
    if runs.is_empty() { bail!("scenario needs at least one user message"); }
    Ok(runs)
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
    let mut scenario_path: Option<PathBuf> = None;
    let mut session_file: Option<PathBuf> = None;
    let mut commands: Vec<(String, Vec<String>)> = Vec::new();
    let mut provider_flag: Option<String> = None;
    let mut model_id: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let val = |args: &Vec<String>, i: &mut usize| -> Result<String> {
            *i += 1;
            args.get(*i).cloned().context("missing value for argument")
        };
        match args[i].as_str() {
            "--extension-dir" => ext_dirs.push(val(&args, &mut i)?),
            "--agent-dir" => agent_dir = Some(PathBuf::from(val(&args, &mut i)?)),
            "--scenario" => scenario_path = Some(PathBuf::from(val(&args, &mut i)?)),
            "--session-file" => session_file = Some(PathBuf::from(val(&args, &mut i)?)),
            "--provider" => provider_flag = Some(val(&args, &mut i)?),
            "--model" => model_id = Some(val(&args, &mut i)?),
            "--prompt" => prompt = Some(val(&args, &mut i)?),
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
            other => bail!("unknown argument: {other}\nusage: pi-rust --extension-dir <p>... [--agent-dir <p>] [--command <name> [args...]]... --scenario <s> [--session-file <p>] [--provider fake|openai|anthropic] [--model <id>] [--prompt <text>]"),
        }
        i += 1;
    }
    let cwd = std::env::current_dir().context("cwd")?;
    let runs = match (&scenario_path, &prompt) {
        (Some(_), Some(_)) => bail!("--scenario and --prompt are mutually exclusive"),
        (Some(p), None) => {
            let raw = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            parse_scenario(&serde_json::from_str(&raw).context("scenario must be a JSON object")?)?
        }
        (None, Some(text)) => vec![RunSpec { text: text.clone(), steps: vec![] }],
        (None, None) => bail!("--scenario <file> or --prompt <text> is required"),
    };

    // Isolated agent dir (owned by the core when defaulted) so extension
    // discovery stays deterministic. Upstream semantics: ctx.cwd is the
    // project directory (the child inherits the core's cwd); the agent dir
    // is discovery-only.
    let agent_dir = agent_dir.unwrap_or_else(|| std::env::temp_dir().join(format!("pi-rust-agent-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&agent_dir).with_context(|| format!("creating agent dir {}", agent_dir.display()))?;

    // M3/WP-301: agent settings from the agent dir (global file only — the
    // project `cwd/.pi/settings.json` merge needs a trust model, out of
    // scope for the headless core). Flags beat settings; settings beat the
    // fake-driver default.
    let agent_config = crate::config::load(&agent_dir).with_context(|| "loading agent config")?;
    let (provider_name, model_id) =
        crate::config::resolve_provider_flags(provider_flag.as_deref(), model_id.as_deref(), &agent_config)?;
    // S3: provider credentials come from env only; the fake driver needs none.
    let provider: Option<ProviderDriver> = build_provider(&provider_name, model_id.as_deref(), &agent_dir)?;
    let fake_mode = provider.is_none();

    // Session file: --session-file, else the upstream layout under the
    // agent dir (`<agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl`).
    let session_file = session_file.unwrap_or_else(|| {
        let dir = agent_dir.join("sessions").join(crate::session::encode_cwd(&cwd.to_string_lossy()));
        std::fs::create_dir_all(&dir).ok();
        let ts = crate::session::now_iso().replace(":", "-");
        dir.join(format!("{}_{}.jsonl", ts, uuid::Uuid::new_v4()))
    });
    let session = Arc::new(Session::create(session_file, &cwd.to_string_lossy()).context("creating session file")?);

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
    eprintln!("[core] spawning sidecar: {node} {}", sidecar.display());

    let log = EventLog::start();
    let fail: Arc<Mutex<Option<String>>> = Arc::default();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    // Child inherits the core's cwd (upstream: ctx.cwd = project dir).
    let (child, tx, rx) = transport::spawn(&node, &sidecar, &sidecar_args, &cwd).await?;
    let (client, events_rx) = BridgeClient::connect(child, tx, rx);
    let client = Arc::new(client);

    // Handshake: init → success response → ready event → state Running.
    let init = json!({"protocolVersion":0,"capabilities":{"ui":["select","confirm","input","editor","notify"],"timeoutMs":INIT_TIMEOUT_MS}});
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
        ready: Mutex::new(Some(ready_tx)),
        log: log.clone(),
        fail: fail.clone(),
        model: Arc::new(Mutex::new((Value::Null, agent_config.default_thinking_level.clone()))),
        msg_tx: msg_tx.clone(),
        done: Mutex::new(Some(done_tx)),
        provider,
    });
    tokio::spawn(crate::agent::event_task(events_rx, ctx.clone()));
    tokio::spawn(crate::agent::agent_task(ctx.clone(), msg_rx));

    let ready = tokio::time::timeout(Duration::from_millis(INIT_TIMEOUT_MS), ready_rx)
        .await.context("sidecar did not send `ready` in time")?.context("sidecar event task exited before `ready`")?;
    if ready.get("apiVersion").and_then(Value::as_u64) != Some(0) {
        bail!("sidecar apiVersion != 0");
    }
    for e in ready.get("errors").and_then(Value::as_array).into_iter().flatten() {
        eprintln!("[core] extension load error: {e}");
    }
    client.set_running();
    {
        let ev = ctx.events.lock().unwrap().clone();
        eprintln!("[core] running: subscribed events {ev:?}");
        if ev.contains("session_start") {
            client.fire(json!({"type":"session_start","event":{"type":"session_start","reason":"startup"}}));
        }
    }

    // --command: run each registered extension command after ready, before the
    // scenario. Unknown command or handler error logs ok:false + error and
    // continues (does not abort the run).
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

    // Seed the runs: one agent run per user message. Fake mode validates each
    // model_script up front; provider mode ignores scripts (real completions).
    for r in &runs {
        if fake_mode {
            FakeDriver::from_steps(&r.steps).with_context(|| "parsing model_script")?;
        }
        let source: &'static str = if r.steps.is_empty() && scenario_path.is_none() { "cli" } else { "rpc" };
        msg_tx.send(Item::User(QueuedUser { text: r.text.clone(), source, steps: r.steps.clone() })).unwrap();
    }
    msg_tx.send(Item::End).unwrap();

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
                eprintln!("[core] sidecar did not exit 10s after session_shutdown");
                fail.lock().unwrap().get_or_insert("sidecar did not exit after session_shutdown".into());
            }
        }
    } else {
        client.set_stopped();
    }

    // Final event: the full session (acked: written+flushed before exit).
    log.emit_final(json!({"kind":"transcript","session":session.dump()})).await;

    // 0 = clean; 2 = scenario/model-script or shutdown failure; 3 = degraded
    // (sidecar crashed mid-run — protocol: degraded is terminal, no respawn).
    Ok(if client.degraded_seen() { 3 } else if fail.lock().unwrap().is_some() { 2 } else { 0 })
}
