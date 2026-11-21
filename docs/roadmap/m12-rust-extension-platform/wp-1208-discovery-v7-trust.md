# WP-1208 — Core-owned discovery + protocol v7 + trust

**Milestone:** M12 · **Story:** E1S3 · **Size:** M · **Status:** open · **Depends on:** WP-1207

## Goal
Move extension discovery to the core ([ADR 0004](../../adr/0004-rust-extensions-share-discovery-and-trust.md)):
classify each candidate, speak bridge **v7** with the sidecar, and gate
Rust user extensions under the WP-1102 trust model.

## Scope
- Core scans `<cwd>/.pi/extensions`, the agent dir (`PI_CODING_AGENT_DIR`)
  and `--extension-dir` entries; classification: a directory with
  `pi-ext.toml` → Rust peer, TS shapes (files/dirs) → sidecar list.
- `pi-ext.toml` manifest: `name`, `description`, `bin` (string, or a `[bin]`
  table keyed by target triple).
- Bridge **v7** (delta only): `init` gains
  `extensions: [{ path, kind: "ts" }]`; the sidecar loads exactly the given
  list (its own discovery is disabled); `protocolVersion: 7`.
- Trust: project-scope Rust extensions inert unless their path is trusted
  (reuse the WP-1102 resolution — no fork of the trust gate).

## Acceptance
- Trust e2e: untrusted project Rust extension → no spawn, zero effect;
  trust granted → loads and works (pattern of `tests/trust.rs`).
- Sidecar e2e with the explicit list: v7 handshake green, the unmodified-TS
  matrix unchanged.
- A manifest with a missing/non-executable `bin` → clean error response, no
  crash.

## Out of scope
- Core-extension loading (WP-1210).
