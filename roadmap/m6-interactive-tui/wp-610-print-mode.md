# WP-610 — Print/JSON output modes

**Milestone:** M6 · **Size:** S · **Status:** open · **Depends on:** M2

## Goal
Upstream `--print` / json-event output parity: one-shot headless runs with
clean stdout variants (raw text, JSON events, final message only).

## Acceptance
- Per-mode golden tests on a scripted run: exact stdout; exit codes per
  upstream; stderr stays human-only.

## Out of scope
- Watch/follow modes.
