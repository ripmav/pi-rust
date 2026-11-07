use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot, watch};

/// Blocking-command timeout, negotiated in the `init` capabilities (v0 default).
pub const INIT_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Running,
    Degraded,
    Stopped,
}

/// Outcome of a blocking bridge command.
pub enum Decision {
    /// The wire response line (`success`, `data` / `error` are inspectable).
    Response(Value),
    /// No answer (timeout or transport loss): caller resolves to the protocol default.
    Unavailable,
}

/// Bridge state machine per docs/bridge-protocol.md §Error & lifecycle:
/// `Running` after the handshake; `Degraded` on a transport error or an
/// unanswered blocking command (built-ins keep working, extension tools are
/// dropped from the model's tool set, decisions resolve to protocol defaults,
/// writes are dropped and logged, no respawn); `Stopped` on clean shutdown
/// or a failed handshake.
pub struct BridgeClient {
    tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    state_tx: watch::Sender<State>,
    state_rx: watch::Receiver<State>,
    exit_rx: watch::Receiver<Option<i32>>,
}

impl BridgeClient {
    pub fn connect(
        mut child: Child,
        tx: mpsc::UnboundedSender<String>,
        mut rx: mpsc::UnboundedReceiver<Value>,
    ) -> (Self, mpsc::UnboundedReceiver<Value>) {
        let (events, events_rx) = mpsc::unbounded_channel::<Value>();
        let (state_tx, state_rx) = watch::channel(State::Stopped);
        let (exit_tx, exit_rx) = watch::channel(None::<i32>);
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));

        // Demux: responses correlate strictly by id (a response for an unknown
        // or already-timed-out id is ignored — protocol rule); everything else
        // is a sidecar→core event.
        let (p, st) = (pending.clone(), state_tx.clone());
        tokio::spawn(async move {
            while let Some(v) = rx.recv().await {
                if v.get("type").and_then(Value::as_str) == Some("response") {
                    if let Some(id) = v.get("id").and_then(Value::as_str) {
                        if let Some(w) = p.lock().unwrap().remove(id) {
                            let _ = w.send(v);
                        }
                    }
                } else {
                    let _ = events.send(v);
                }
            }
            Self::degrade(&st); // stdout EOF: transport error unless already stopped
        });

        // Child exit: an unexpected exit is a transport error; the exit code is
        // recorded so a clean shutdown can be verified.
        let st2 = state_tx.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            let _ = exit_tx.send_replace(code);
            Self::degrade(&st2);
        });

        (
            Self { tx, pending, state_tx, state_rx, exit_rx },
            events_rx,
        )
    }

    fn degrade(st: &watch::Sender<State>) {
        if *st.borrow() == State::Running {
            eprintln!("[core] bridge DEGRADED: sidecar transport lost; built-ins continue, extension tools unavailable, no respawn");
            let _ = st.send(State::Degraded);
        }
    }

    pub fn state(&self) -> State {
        *self.state_rx.borrow()
    }

    pub fn set_running(&self) {
        let _ = self.state_tx.send(State::Running);
    }

    pub fn set_stopped(&self) {
        let _ = self.state_tx.send(State::Stopped);
    }

    /// Blocking command: sends `{id, type, ...payload}`; exactly one response
    /// line matches the id; per-request timeout = the negotiated init timeoutMs.
    pub async fn request(&self, ty: &str, payload: Value) -> Decision {
        if self.state() != State::Running {
            eprintln!("[core] {ty} dropped (bridge {:?}): protocol default applies", self.state());
            return Decision::Unavailable;
        }
        self.request_inner(ty, payload).await
    }

    /// Handshake command: sent before `ready` (state not yet Running), so it
    /// bypasses the state gate; every other command still goes through `request`.
    pub async fn request_init(&self, ty: &str, payload: Value) -> Decision {
        self.request_inner(ty, payload).await
    }

    async fn request_inner(&self, ty: &str, mut payload: Value) -> Decision {
        let id = uuid::Uuid::new_v4().to_string();
        let (otx, orx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), otx);
        payload["id"] = json!(id);
        payload["type"] = json!(ty);
        if self.tx.send(payload.to_string()).is_err() {
            self.pending.lock().unwrap().remove(&id);
            Self::degrade(&self.state_tx);
            return Decision::Unavailable;
        }
        match tokio::time::timeout(Duration::from_millis(INIT_TIMEOUT_MS), orx).await {
            Ok(Ok(resp)) => Decision::Response(resp),
            // Timeout: blocking command unanswered after the init timeoutMs.
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Self::degrade(&self.state_tx);
                Decision::Unavailable
            }
        }
    }

    /// Fire-and-forget command line (no response expected). Dropped and logged
    /// in `Degraded`/`Stopped`.
    pub fn fire(&self, line: Value) {
        if self.state() != State::Running {
            eprintln!("[core] F&F dropped (bridge {:?}): {}", self.state(), line.get("type").and_then(Value::as_str).unwrap_or("?"));
            return;
        }
        if self.tx.send(line.to_string()).is_err() {
            Self::degrade(&self.state_tx);
        }
    }

    /// Reply line for a sidecar→core request (tool_call_request,
    /// set_model_request, set_thinking_level_request) or an extension_ui_response.
    pub fn send(&self, line: Value) {
        if self.state() != State::Running {
            eprintln!("[core] reply dropped (bridge {:?}): {}", self.state(), line.get("type").and_then(Value::as_str).unwrap_or("?"));
            return;
        }
        if self.tx.send(line.to_string()).is_err() {
            Self::degrade(&self.state_tx);
        }
    }

    /// Resolves with the sidecar exit code once the process has exited.
    pub async fn wait_exited(&self) -> Option<i32> {
        let mut rx = self.exit_rx.clone();
        while rx.borrow().is_none() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let x = *rx.borrow();
        x
    }
}
