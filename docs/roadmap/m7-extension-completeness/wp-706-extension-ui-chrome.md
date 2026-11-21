# WP-706 — Extension UI chrome (status/title/banners)

**Milestone:** M7 · **Size:** S · **Status:** open · **Depends on:** 601

## Goal
`setStatus`/`setTitle` (already bridged) render in TUI chrome; any
extension-contributed UI pieces present at the pinned commit render or are
explicitly documented as non-goals.

## Acceptance
- Fixture: status text appears in status bar, title in title area; cleared
  on session end; matrix re-run.

## Out of scope
- Arbitrary extension theming.
