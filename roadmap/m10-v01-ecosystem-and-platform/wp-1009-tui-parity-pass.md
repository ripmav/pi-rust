# WP-1009 — TUI parity pass vs upstream p2

**Milestone:** M10 · **Size:** M · **Status:** planned · **Depends on:** 1001

## Goal
Close the functional TUI gaps that opened when upstream moved past the
pin: the p2 work (light/dark theme cross-fade, app-wide appearance setting,
per-conversation-location MCP) plus the documented v0 TUI deltas, within the
functional-parity standard (pixel-perfect stays a non-goal).

## Scope
- Audit at the new pin (1001): diff the interactive surface — theme
  handling (upstream `theme-tokens.ts`/`oklab.ts` changes since the pin),
  appearance/light-dark handling, keybindings deltas, MCP per-location
  semantics vs the agent-dir `mcp.json` model.
- Implement the top functional gaps, prioritized: (1) light/dark theme
  handling if the TUI can't express it (theme system exists from WP-607 —
  extend, don't rebuild), (2) MCP location semantics where an unmodified
  extension depends on them, (3) keybinding parity deltas.
- TUI hardening sweep of the documented LOWs: WP-906 BashCapture leak on
  pane close (bounded; fix the drop), WP-606/708 logged-ignored list —
  re-evaluate each entry, implement what is cheap, re-document the rest.
- PTY matrix re-run (3 PTY tests + any new parity tests).

## Acceptance
- Per-item table in this file (gap, upstream ref, closed/documented,
  rationale) — the same evaluation-memo style as M7.
- Light/dark or appearance change (if implemented) demonstrable in the PTY
  harness (theme switch visible).
- PTY + full suite green; matrix green.

## Out of scope
- Pixel-perfect Ink/React parity (vision non-goal, unchanged).
- Per-location MCP as a product feature for core config (extension-facing
  semantics only).
