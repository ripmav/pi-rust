# WP-1210 — Core-extension loading in the core

**Milestone:** M12 · **Story:** E1S3 · **Size:** M · **Status:** open · **Depends on:** WP-1206, WP-1209

## Goal
Wire in-process core extensions into the real core
([ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md)):
`dlopen` at startup, verify, and dispatch in the agent loop.

## Scope
- After config + trust resolution, before the first session event: discover
  `<install>/core-ext/` (override `PI_RUST_CORE_EXT_DIR`, mirroring
  `find_sidecar`), verify the manifest sha256, check
  `PI_EXT_CORE_API_VERSION`, `dlopen` in name order.
- In-process registry (tools/commands/flags/events); dispatch in the agent
  loop with architecture decision order (core extensions first).
- Faulted library → skipped + logged (session continues); sha256 mismatch
  → refused + logged.
- Install layout: `core-ext/libpi-ext-<name>.{so,dylib,dll}` +
  `manifest.json` (name, `apiVersion`, sha256 per library).
- CI: dlopen smoke per library; the packed `core-ext/` payload in
  release artifacts + `install.sh` unpacking.

## Acceptance
- A Rust-only run spawns no Node process (test asserts no sidecar spawn).
- `hello`'s e2e green in the real core (same scenario as the original).
- A faulted example library → session continues, other libraries work.
- Release artifact contains `core-ext/` + manifest; `install.sh` verified
  by the fresh-clone/release checks.

## Out of scope
- The remaining ports (WP-1212–1216); authoring tooling (WP-1217).
