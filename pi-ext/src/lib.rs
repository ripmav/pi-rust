//! `pi-ext` — public Rust extension framework for pi (skeleton).
//!
//! The final surface mirrors the upstream extension API (`pi` + `ctx`,
//! ADR 0002) for both extension targets: v6-speaking user binaries (ADR 0001)
//! and in-process core cdylibs (ADR 0006). The API modules — traits/types,
//! bridge v6 wire types, runtime glue — land in WP-1202..WP-1206; this crate
//! is the build/test skeleton (WP-1201, ADR 0003).

pub mod placeholder;
