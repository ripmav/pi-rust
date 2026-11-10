# WP-306 — Default agent dir = `~/.pi/agent`

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** 301, 304

## Goal
Flip the M1 isolation default: with `~/.pi/agent` format-compatible, pi-rust
uses it by default (global extensions load "wie gewohnt"); `--isolated`
keeps the temp-dir behavior for tests/CI.

## Acceptance
- Default run loads `~/.pi/agent/extensions/*` (integration test with a
  fixture agent dir via `PI_*` env override, not the real home); e2e matrix
  unchanged with `--isolated`.

## Out of scope
- Per-project agent dirs beyond upstream semantics.
