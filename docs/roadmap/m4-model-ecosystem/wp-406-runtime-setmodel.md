# WP-406 — Runtime `setModel` switching

**Milestone:** M4 · **Size:** M · **Status:** open · **Depends on:** 302, 303

## Goal
`ctx.setModel` from extensions (and later TUI) actually switches the active
driver mid-session: new provider/model/thinking level apply to the next
turn; sidecar's optimistic local update stays gated on the core ack (M1
behavior) — now the ack is real.

## Acceptance
- E2E: scenario run with an extension calling `setModel` mid-run; transcript
  shows the model change; invalid model id → request declined, view
  unchanged; `getSettings`/`pi.model` reflect the switch.

## Out of scope
- Per-message model overrides.
