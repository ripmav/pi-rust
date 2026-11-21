# WP-1202 — `pi-ext`: bridge wire types

**Milestone:** M12 · **Story:** E1S2 · **Size:** S · **Status:** open · **Depends on:** WP-1201

## Goal
One source of truth for the bridge v6 wire types in `pi-ext`
([ADR 0003](../../adr/0003-workspace-with-pi-ext-crate.md)): envelope,
`init`/`ready` payloads, and every catalog event payload.

## Scope
- Command/response/event line types (framing rules per
  [bridge-protocol.md](../../bridge-protocol.md) §Framing).
- `init` (`protocolVersion`, `capabilities`, `mcpConfig`, `keybindings`,
  `projectTrusted`) + `ready` (extensions, tools, commands, shortcuts,
  shortcuts diagnostics, flags, providers, events).
- Event payload types for the full catalog (F&F + decision) and result types
  (block/reason, message injection, `systemPrompt` mutation, context
  filtering, compaction results, …).
- Ported from the hand-rolled `json!` in `src/agent.rs`; fixtures from the
  wire examples in bridge-protocol.md.

## Acceptance
- Serde round-trip unit tests against the wire examples in bridge-protocol.md.
- Types compile in the standalone crate (no core-crate imports).
- Public types have rustdoc.

## Out of scope
- Protocol v7 (`init.extensions` — WP-1208); the `pi` + `ctx` API surface
  (WP-1203/1204).
