//! `pi-ext` — public Rust extension framework for pi.
//!
//! The final surface mirrors the upstream extension API (`pi` + `ctx`,
//! ADR 0002) for both extension targets: v6-speaking user binaries
//! (ADR 0001) and in-process core cdylibs (ADR 0006). Landed so far:
//! [`wire`] — the bridge protocol v6 wire types, one source of truth for
//! the core's server side and the extension's client side (WP-1202,
//! ADR 0003); [`pi`] — the `pi` object + `ctx` core API (WP-1203,
//! ADR 0002): the [`pi::Extension`] trait, [`pi::Tool`] registration
//! with [`pi::schema`] helpers, and the message/session/model surface.
//! UI/commands/providers (WP-1204) and the transports
//! (WP-1205/1206) follow.

pub mod pi;
pub mod wire;
