# WP-607 — Themes

**Milestone:** M6 · **Size:** S · **Status:** open · **Depends on:** 601

## Goal
Load upstream theme definitions (built-in + `~/.pi` custom themes dir) and
map to ratatui styles; theme switching at runtime (TUI command +
`setTheme` extension hook if present at pinned commit).

## Acceptance
- Golden: each built-in theme renders a fixture screen with expected colors
  (buffer snapshot); custom theme file loads; unknown theme → error.

## Out of scope
- Theme authoring UI.
