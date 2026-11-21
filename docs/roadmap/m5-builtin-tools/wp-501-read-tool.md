# WP-501 — `read` tool

**Milestone:** M5 · **Size:** S · **Status:** open · **Depends on:** —

## Goal
Built-in `read` with upstream semantics: file contents, `offset`/`limit`,
50KB/2000-line truncation contract, image files detected (WP-408 consumes).

## Acceptance
- Golden tests: exact stdout for truncated file, binary detection, missing
  file error text matches upstream; extension matrix re-run (tools visible
  unchanged).

## Out of scope
- Notebooks/structured formats beyond upstream v1 behavior.
