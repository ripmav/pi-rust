# WP-1216 — Port `plan-mode` + porting guide

**Milestone:** M12 · **Story:** E1S4 · **Size:** L · **Status:** open · **Depends on:** WP-1210

## Goal
The deep-API port (598 lines upstream — the only reference that exercises
the full surface) plus the porting guide that closes E1S4
([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).

## Scope
- `core-ext/plan-mode`: `registerFlag("plan")`, commands `plan`/`todos`,
  shortcut Ctrl+Alt+P, bash allowlist block on `tool_call`, `context`
  message filtering, `before_agent_start` message injection
  (`[PLAN MODE ACTIVE]` / execution context), `[DONE:n]` tracking on
  `turn_end`, `agent_end` plan extraction + completion, tool-set
  management (`getActiveTools`/`setActiveTools`), `appendEntry`
  persistence + resume-restore (incl. the post-execute-marker scan),
  `sendMessage`/`sendUserMessage` (`deliverAs`, `triggerTurn`), UI
  (`select`, `editor`, `setStatus`, `setWidget`, theme functions).
- **Porting guide** in `docs/`: per-extension mapping table (upstream API
  → `pi-ext` API) for all six ports, hard parts annotated (plan-mode's
  resume scan, with-deps' dependency mapping), and the general recipe for
  porting any TS extension.

## Acceptance
- Same e2e scenarios as the unmodified original: `--command plan` toggle,
  bash allowlist block, `[PLAN MODE ACTIVE]` injection, `appendEntry`
  persistence + resume, TUI widget/status.
- Porting guide exists in `docs/` and links all six `core-ext/` crates.
