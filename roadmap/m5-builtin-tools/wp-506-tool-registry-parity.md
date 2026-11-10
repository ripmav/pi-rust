# WP-506 — Tool registry & schema parity

**Milestone:** M5 · **Size:** S · **Status:** open · **Depends on:** 501–504

## Goal
Core tool registry exposes upstream names/descriptions/JSON schemas exactly
(what the model and extensions see); `ready.tools` passthrough + built-ins
merged with upstream precedence (extension shadowing rules).

## Acceptance
- Schema-diff test: built-in tool JSON equals upstream-generated schemas
  (golden files from upstream at the pinned commit); shadowing test.

## Out of scope
- Tool-search style dynamic loading (M7 bundled-extension evaluation).
