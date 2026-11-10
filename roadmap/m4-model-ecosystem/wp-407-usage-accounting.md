# WP-407 — Token & cost accounting

**Milestone:** M4 · **Size:** S · **Status:** open · **Depends on:** 302

## Goal
Every model response records usage (input/output/cache tokens, cost per
model registry prices) into the session; cumulative totals available to the
TUI and `--print-usage` headless flag.

## Acceptance
- Usage lands in session entries (round-trip via WP-304 format); totals test
  across multi-turn + tool-call runs; unknown price → 0 + no crash.

## Out of scope
- Budgets/limits enforcement.
