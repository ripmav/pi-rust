# WP-307 — Prompt templates (data path)

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** 301

## Goal
Discover and load upstream prompt templates (agent dir + project dir) so the
TUI command menu can offer them; headless: `--template <name> [args]` expands
to a prompt.

## Acceptance
- Discovery test (project beats agent dir); expansion test with `$ARGUMENTS`
  semantics per upstream; unknown template → error listing available.

## Out of scope
- TUI rendering (WP-608).
