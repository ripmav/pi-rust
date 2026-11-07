use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::agent::{Core, EventLog, Item, QueuedUser};
use crate::client::{BridgeClient, Decision, State, INIT_TIMEOUT_MS};
use crate::model::FakeDriver;
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
    let mut dir = cwd.to_path_buf();
    for _ in 0..6 {
        let cand = dir.join("sidecar").join("index.mjs");
        if cand.is_file() {
            return Ok(cand);
        }
        if !dir.pop() {
            break;
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
            other => bail!("unknown argument: {other}\nusage: pi-rust --extension-dir <p>... [--agent-dir <p>] --scenario <s> [--session-file <p>]"),
        }
        i += 1;
    }
    let cwd = std::env::current_dir().context("cwd")?;
    let spath = scenario_path.clone().context("--scenario <file> is required")?;
    let raw = std::fs::read_to_string(&spath).with_context(|| format!("reading {}", spath.display()))?;
    let runs = parse_scenario(&serde_json::from_str(&raw).context("scenario must be a JSON object")?)?;

    // Isolated agent dir (owned by the core when defaulted) so extension
    // discovery stays deterministic; the sidecar's cwd is the agent dir.
    let agent_dir = agent_dir.unwrap_or_else(|| std::env::temp_dir().join(format!("pi-rust-agent-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&agent_dir).with_context(|| format!("creating agent dir {}", agent_dir.display()))?;

    // v0 session file: --session-file or .pi-rust/sessions/<uuid>.jsonl.
    let session_file = session_file.unwrap_or_else(|| PathBuf::from(".pi-rust").join("sessions").join(format!("{}.jsonl", uuid::Uuid::new_v4())));
    let session = Arc::new(Session::open(session_file)?);

    let sidecar = find_sidecar(&cwd)?;
    let node = std::env::var("PI_RUST_NODE").unwrap_or_else(|_| "node".into());
    let mut sidecar_args: Vec<String> = Vec::new();
    for d in &ext_dirs {
        // The sidecar's cwd is the isolated agent dir, so make relative paths absolute first.
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

    let (child, tx, rx) = transport::spawn(&node, &sidecar, &sidecar_args, &agent_dir).await?;
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
        model: Arc::default(),
        msg_tx: msg_tx.clone(),
        done: Mutex::new(Some(done_tx)),
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

    // Seed the scenario: one agent run per user message with its own script.
    for r in &runs {
        FakeDriver::from_steps(&r.steps).with_context(|| "parsing model_script")?;
        msg_tx.send(Item::User(QueuedUser { text: r.text.clone(), source: "rpc", steps: r.steps.clone() })).unwrap();
    }
    msg_tx.send(Item::End).unwrap();

    // Wait for the scenario (and any extension-injected user messages).
    let _ = done_rx.await;

    // Clean shutdown: session_shutdown → sidecar exits 0. The core marks the
    // state Stopped once the shutdown is sent (avoids misclassifying the
    // normal-exit race); a non-zero sidecar exit is still recorded as a failure.
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

    // Final event: the full session (acked: written+flushed before exit).
    log.emit_final(json!({"kind":"transcript","session":session.dump()})).await;

    // 0 = clean; 2 = scenario/model-script failure; 3 = degraded/crashed.
    let state = client.state();
    Ok(if fail.lock().unwrap().is_some() { 2 } else if state != State::Stopped { 3 } else { 0 })
}
