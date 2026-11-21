# WP-1205 — `pi-ext`: user target (v6 process)

**Milestone:** M12 · **Story:** E1S2 · **Size:** M · **Status:** open · **Depends on:** WP-1203, WP-1204

## Goal
Turn an extension's `main` into a bridge-v6-speaking process
([ADR 0001](../../adr/0001-rust-extensions-as-bridge-subprocesses.md)) —
the runtime glue for user Rust extensions.

## Scope
- Stdio NDJSON framing (bridge-protocol.md §Framing), `init` handshake
  (validate `protocolVersion`, answer `apiVersion`, emit `ready` with the
  extension's catalog).
- Dispatch loop: fire-and-forget events (unknown → logged, never fatal) +
  decision commands (run handlers, respond with the combined result, respect
  the capability timeout).
- UI request round-trip (`extension_ui_request` / `extension_ui_response`),
  provider-stream events.
- Panic safety: a handler panic → clean error response, no half-lines.
- Example extension (tool + command + event handler) + NDJSON test harness
  (same pattern as `tests/extension_bridge.rs`).

## Acceptance
- Harness e2e: `ready` catalog correct; tool round-trip; decision block;
  UI dialog round-trip.
- Malformed-line behavior matches the protocol (parse error response);
  unknown commands → protocol error, not a crash.

## Out of scope
- The core target (WP-1206); the core-side runtime (WP-1207+).
