# WP-601 — TUI skeleton (ratatui)

**Milestone:** M6 · **Size:** M · **Status:** open · **Depends on:** M3

## Goal
Interactive mode: chat pane (scrolling transcript), input line, status bar
(model, cwd, session). Replaces headless-only runs; `pi-rust` with no
`--scenario/--prompt` enters the TUI.

## Acceptance
- Integration smoke: enter TUI, send message, scripted fake model replies
  rendered, `q`/exit works; no raw panics on resize (resize handler test).

## Out of scope
- Anything beyond skeleton (each feature = its own WP).
