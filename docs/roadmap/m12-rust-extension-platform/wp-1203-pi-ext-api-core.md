# WP-1203 — `pi-ext`: API surface — core

**Milestone:** M12 · **Story:** E1S2 · **Size:** M · **Status:** in progress (branch `feat/wp-1203-pi-ext-api-core`) · **Depends on:** WP-1201, WP-1202

## Goal
The core of the upstream `pi` + `ctx` mirror
([ADR 0002](../../adr/0002-mirror-full-upstream-extension-api.md)): the
`Extension` trait, the context, tools, and the message/session surface.

## Scope
- `Extension` trait (the init entry) + context types (`cwd`, session view,
  `hasUI`).
- `on(event)` for all catalog events — typed handler signatures, decision vs
  fire-and-forget.
- `registerTool` (JSON-schema parameters, label/description, `exposure`,
  `defaultActive`, `execute`).
- `sendUserMessage`, `appendEntry`, session name + label, `exec`,
  `getSettings`, `setModel`, thinking-level selection.
- Doc examples per area.

## Acceptance
- `cargo test --doc` green; a documented "hello tool" example compiles
  verbatim from the docs.
- All surfaces above exist and are documented; unit tests for the tool-schema
  helpers.

## Out of scope
- UI / providers / MCP / commands / shortcuts / flags (WP-1204); transports
  (WP-1205/1206).
