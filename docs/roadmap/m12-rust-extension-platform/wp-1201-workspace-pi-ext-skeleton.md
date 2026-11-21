# WP-1201 — Workspace + `pi-ext` skeleton

**Milestone:** M12 · **Story:** E1S2 · **Size:** S · **Status:** in progress (branch `feat/wp-1201-pi-ext-skeleton`) · **Depends on:** —

## Goal
Make the repo a Cargo workspace and create the empty public `pi-ext` crate
([ADR 0003](../../adr/0003-workspace-with-pi-ext-crate.md)) — the foundation for all
later WPs. No behavior change.

## Scope
- Root `Cargo.toml`: add `[workspace]` (members: root package `pi-rust`,
  `pi-ext`); the root stays the binary; all existing paths unchanged.
- New `pi-ext/` lib crate: 0.0.0, deps serde + serde_json only, one
  placeholder module + a passing test.
- CI: jobs build/test the workspace (invocations stay at the root).

## Acceptance
- `cargo build` / `cargo test` at the root cover both crates;
  `cargo test -p pi-ext` works.
- Full suite + e2e matrix green; clippy + fmt clean; CI 7/7.

## Out of scope
- Any API, wire types or runtime (WP-1202+).
