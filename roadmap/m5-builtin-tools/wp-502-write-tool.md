# WP-502 — `write` tool

**Milestone:** M5 · **Size:** S · **Status:** open · **Depends on:** —

## Goal
Built-in `write`: full-file overwrite/create with parent-dir creation,
upstream result message format.

## Acceptance
- Unit tests: create, overwrite, nested dirs; result summary format matches
  upstream string; permission error mapped.

## Out of scope
- Write guards/confirmation (WP-505 territory).
