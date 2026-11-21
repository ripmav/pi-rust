# WP-608 — Slash commands in TUI

**Milestone:** M6 · **Size:** S · **Status:** open · **Depends on:** 605, 307

## Goal
`/…` menu in the editor: built-in commands (upstream set at pinned commit)
+ extension `registerCommand` commands (already bridged, M1) + prompt
templates (WP-307).

## Acceptance
- Menu shows extension command (fixture) + template; invocation runs the
  bridge command path (M1 behavior) with TUI result display; unknown →
  error list.

## Out of scope
- Command history beyond editor history.
