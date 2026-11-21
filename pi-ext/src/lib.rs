//! `pi-ext` — public Rust extension framework for pi.
//!
//! The final surface mirrors the upstream extension API (`pi` + `ctx`,
//! ADR 0002) for both extension targets: v6-speaking user binaries
//! (ADR 0001) and in-process core cdylibs (ADR 0006). Landed so far:
//! [`wire`] — the bridge protocol v6 wire types, one source of truth for
//! the core's server side and the extension's client side (WP-1202,
//! ADR 0003). The `pi`/`ctx` API modules and runtime glue land in
//! WP-1203..WP-1206.

pub mod wire;
