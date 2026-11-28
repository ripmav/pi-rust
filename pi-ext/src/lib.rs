//! `pi-ext` — public Rust extension framework for pi.
//!
//! The final surface mirrors the upstream extension API (`pi` + `ctx`,
//! ADR 0002) for both extension targets: v6-speaking user binaries
//! (ADR 0001) and in-process core cdylibs (ADR 0006). Landed so far:
//! [`wire`] — the bridge protocol v6 wire types, one source of truth for
//! the core's server side and the extension's client side (WP-1202,
//! ADR 0003); [`pi`] — the full `pi` object + `ctx` mirror (WP-1203/
//! WP-1204, ADR 0002): the [`pi::Extension`] trait, tool registration
//! ([`pi::Tool`] with [`pi::schema`] helpers), commands/shortcuts/flags,
//! provider + MCP registration, the UI surface ([`pi::ExtensionUI`])
//! and the message/session/model surface. The catalog ↔ API parity test
//! (`pi-ext/tests/parity.rs`) enforces the mirror against the bridge v6
//! catalog; [`process`] is the v6 user process runtime (WP-1205) that
//! turns an [`pi::Extension`] into a bridge-v6-speaking process on
//! stdio — the core cdylib target follows in WP-1206.

pub mod pi;
pub mod process;
pub mod wire;
