# WP-1207 — N-peer runtime in the core

**Milestone:** M12 · **Story:** E1S3 · **Size:** M · **Status:** open · **Depends on:** WP-1205

## Goal
Generalize the 1:1 `BridgeClient` to N bridge peers
([ADR 0001](../../adr/0001-rust-extensions-as-bridge-subprocesses.md)):
the sidecar plus N Rust extension subprocesses, one state machine per peer.

## Scope
- Refactor `BridgeClient` → per-peer state machine + peer set (the sidecar
  becomes peer #1 with unchanged behavior).
- Spawn N Rust extension peers in parallel; per-peer `init`/`ready`
  handshake; per-peer `Running` / `Degraded` / `Stopped` (no respawn).
- `ready` aggregation across peers (events, tools, commands, shortcuts,
  flags, providers).
- Fire-and-forget fan-out to subscribed peers; decision combination in
  architecture order (core → sidecar → user peers path-sorted; first `block`
  wins, other results combined in order).
- Tool merge into the model tool set; name collision → later source's tool
  dropped + diagnostic logged.
- Per-peer `Degraded`: its tools dropped, its decisions resolve to protocol
  defaults, logged; the other peers continue.

## Acceptance
- Integration test: two fake v6 Rust-peer fixtures + the sidecar in one
  session; kill one peer mid-session → only its tools dropped, exit-code
  semantics preserved.
- Full existing suite green (the sidecar path is unchanged).

## Out of scope
- Discovery (WP-1208); in-process core extensions (WP-1210).
