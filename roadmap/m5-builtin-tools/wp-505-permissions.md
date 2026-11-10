# WP-505 — Permission system parity

**Milestone:** M5 · **Size:** M · **Status:** open · **Depends on:** 504, 604

## Goal
Upstream permission behavior: default-allow with extension gate hooks (the
existing `permission-gate.ts` pattern is the contract), later TUI confirm
dialogs for dangerous commands.

## Acceptance
- `permission-gate.ts` unmodified still blocks (matrix re-run); headless
  mode answers per upstream defaults; TUI confirm path (WP-604) shows the
  exact command.

## Out of scope
- Allow/deny rule files beyond upstream v1.
