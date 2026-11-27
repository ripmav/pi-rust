//! The UI surface — upstream `ctx.ui` (the wire-expressible subset per
//! ADR 0002): the dialogs `select`/`confirm`/`input`/`editor`
//! (req/resp `extension_ui_request`), `notify` and the F&F
//! `setStatus`/`setTitle`/`set_editor_text`, plus the `custom` pane
//! (v6, the WP-707 descriptor subset). Implementations are
//! target-specific ([`ExtensionTransport::ui`]); [`NullUi`] is the
//! headless no-UI default — the `init` capability list `ui: []` keeps
//! the upstream noOp UI context: `select`/`input`/`editor` resolve to
//! `None`, `confirm` to `false`, the rest no-op.

use crate::wire::{PaneBlocked, PaneInput, PaneKeyInput, PaneLine, PaneList};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Dialog options (upstream `ExtensionUIDialogOptions` minus the
/// non-JSON `signal`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiDialogOptions {
    /// Milliseconds; omitted = the `init`-negotiated `timeoutMs`.
    pub timeout_ms: Option<u64>,
}

/// `notify` severity (upstream `"info" | "warning" | "error"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    Info,
    Warning,
    Error,
}

impl NotifyKind {
    /// The wire value (`notifyType`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// The UI surface behind `ctx.ui` (upstream `ExtensionUIContext`).
/// Dialogs are blocking; the rest fire-and-forget.
pub trait ExtensionUI: Send + Sync {
    /// Show a selector; `None` on dismiss/timeout/no-UI.
    fn select(
        &self,
        title: &str,
        options: &[String],
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// Show a confirmation dialog; `false` on dismiss/timeout/no-UI.
    fn confirm(
        &self,
        title: &str,
        message: &str,
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
    /// Show a text input; `None` on dismiss/timeout/no-UI.
    fn input(
        &self,
        title: &str,
        placeholder: Option<&str>,
        opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// Show a multi-line editor; `None` on dismiss/no-UI.
    fn editor(
        &self,
        title: &str,
        prefill: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// Show a user-facing notification (F&F).
    fn notify(&self, message: &str, kind: Option<NotifyKind>);
    /// Show a custom pane with keyboard focus (v6): `None` on timeout,
    /// a throwing factory or no `custom` capability (headless).
    fn custom(
        &self,
        factory: PaneFactory,
        blocked: Option<PaneBlocked>,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + '_>>;
    /// Set status-bar text (F&F); `None` clears.
    fn set_status(&self, key: &str, text: Option<&str>);
    /// Set the terminal window/tab title (F&F).
    fn set_title(&self, title: &str);
    /// Set the core input editor text (F&F).
    fn set_editor_text(&self, text: &str);
}

/// The headless no-UI default (upstream noOp UI context; bridge §init
/// capabilities `ui: []`).
#[derive(Default)]
pub struct NullUi;

impl ExtensionUI for NullUi {
    fn select(
        &self,
        _title: &str,
        _options: &[String],
        _opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async { None })
    }
    fn confirm(
        &self,
        _title: &str,
        _message: &str,
        _opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async { false })
    }
    fn input(
        &self,
        _title: &str,
        _placeholder: Option<&str>,
        _opts: &UiDialogOptions,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async { None })
    }
    fn editor(
        &self,
        _title: &str,
        _prefill: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>> {
        Box::pin(async { None })
    }
    fn notify(&self, _message: &str, _kind: Option<NotifyKind>) {}
    fn custom(
        &self,
        _factory: PaneFactory,
        _blocked: Option<PaneBlocked>,
    ) -> Pin<Box<dyn Future<Output = Option<Value>> + Send + '_>> {
        Box::pin(async { None })
    }
    fn set_status(&self, _key: &str, _text: Option<&str>) {}
    fn set_title(&self, _title: &str) {}
    fn set_editor_text(&self, _text: &str) {}
}

/// A pane descriptor snapshot — a full replacement, not a patch (v6
/// `pane_update`; the WP-707 production shapes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaneDescriptor {
    pub title: Option<String>,
    pub lines: Option<Vec<PaneLine>>,
    pub list: Option<PaneList>,
    pub input: Option<PaneInput>,
    pub blocked: Option<PaneBlocked>,
}

/// A pane component (v6 `custom`): the factory receives the [`Pane`]
/// handle, drives the descriptor from it and ends the pane with
/// [`Pane::done`]. The result of `done` is what `custom` resolves to.
pub type PaneFactory = Box<dyn FnOnce(Pane) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The pane connection the transports implement (WP-1205/1206): the
/// `pane_open`/`pane_update`/`pane_close` lines out, `pane_key` in.
pub trait PaneHost: Send + Sync {
    /// Push a descriptor snapshot (extension→core `pane_update`).
    fn update(&self, id: &str, desc: &PaneDescriptor);
    /// Wait for the next TUI key (core→extension `pane_key`).
    fn next_key(&self, id: &str)
        -> Pin<Box<dyn Future<Output = Option<PaneKeyInput>> + Send + '_>>;
    /// Close the pane (extension→core `pane_close`); resolves `custom`
    /// with `result`.
    fn done(&self, id: &str, result: Value);
}

/// A handle to the open pane, handed to the [`PaneFactory`].
pub struct Pane {
    host: Arc<dyn PaneHost>,
    id: String,
}

impl Pane {
    /// Build the pane handle (runtime entry; the transports assign the
    /// pane `id` when the pane opens).
    #[must_use]
    pub fn new(host: Arc<dyn PaneHost>, id: String) -> Self {
        Self { host, id }
    }

    /// The pane id (assigned by the transport).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Push a full descriptor snapshot (replacement, not patch).
    pub fn update(&self, desc: &PaneDescriptor) {
        self.host.update(&self.id, desc);
    }

    /// Wait for the next TUI key; `None` when the pane is gone.
    pub fn next_key(&self) -> Pin<Box<dyn Future<Output = Option<PaneKeyInput>> + Send + '_>> {
        self.host.next_key(&self.id)
    }

    /// End the pane with its result (the `custom` promise resolves to
    /// it).
    pub fn done(&self, result: Value) {
        self.host.done(&self.id, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn block_on<F: std::future::Future + Send>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    #[test]
    fn null_ui_is_the_no_ui_default() {
        // Upstream noOp: select/input/editor → None, confirm → false,
        // custom → None (the factory is never run), the rest no-op.
        let ui = NullUi;
        let opts = UiDialogOptions {
            timeout_ms: Some(1),
        };
        assert_eq!(block_on(ui.select("t", &["a".into()], &opts)), None);
        assert!(!block_on(ui.confirm("t", "m", &opts)));
        assert_eq!(block_on(ui.input("t", None, &opts)), None);
        assert_eq!(block_on(ui.editor("t", Some("x"))), None);
        ui.notify("m", Some(NotifyKind::Error));
        let ran = Arc::new(Mutex::new(false));
        let ran2 = Arc::clone(&ran);
        assert_eq!(
            block_on(ui.custom(
                Box::new(move |_pane: Pane| {
                    *ran2.lock().unwrap() = true;
                    Box::pin(async {})
                }),
                None
            )),
            None
        );
        assert!(!*ran.lock().unwrap());
        ui.set_status("k", None);
        ui.set_title("t");
        ui.set_editor_text("t");
    }

    #[test]
    fn notify_kind_wire_values() {
        assert_eq!(NotifyKind::Info.as_str(), "info");
        assert_eq!(NotifyKind::Warning.as_str(), "warning");
        assert_eq!(NotifyKind::Error.as_str(), "error");
    }

    /// Records the pane calls, for asserting `Pane` → `PaneHost` routing.
    #[derive(Clone, Default)]
    struct FakePaneHost {
        id: String,
        updates: Arc<Mutex<Vec<PaneDescriptor>>>,
        keys: Arc<Mutex<std::collections::VecDeque<PaneKeyInput>>>,
        done: Arc<Mutex<Option<Value>>>,
    }

    impl PaneHost for FakePaneHost {
        fn update(&self, id: &str, desc: &PaneDescriptor) {
            assert_eq!(id, &self.id);
            self.updates.lock().unwrap().push(desc.clone());
        }
        fn next_key(
            &self,
            id: &str,
        ) -> Pin<Box<dyn Future<Output = Option<PaneKeyInput>> + Send + '_>> {
            assert_eq!(id, &self.id);
            Box::pin(async move { self.keys.lock().unwrap().pop_front() })
        }
        fn done(&self, id: &str, result: Value) {
            assert_eq!(id, &self.id);
            *self.done.lock().unwrap() = Some(result);
        }
    }

    #[test]
    fn pane_routes_calls_to_the_host() {
        let host = FakePaneHost {
            id: "p1".into(),
            ..Default::default()
        };
        let pane = Pane::new(Arc::new(host.clone()), "p1".into());
        assert_eq!(pane.id(), "p1");
        pane.update(&PaneDescriptor {
            title: Some("List".into()),
            ..Default::default()
        });
        assert_eq!(host.updates.lock().unwrap().len(), 1);
        host.keys.lock().unwrap().push_back(PaneKeyInput {
            code: "down".into(),
            text: None,
        });
        let key = block_on(pane.next_key()).expect("queued key");
        assert_eq!(key.code, "down");
        assert_eq!(block_on(pane.next_key()), None);
        pane.done(json!("result"));
        assert_eq!(*host.done.lock().unwrap(), Some(json!("result")));
    }
}
