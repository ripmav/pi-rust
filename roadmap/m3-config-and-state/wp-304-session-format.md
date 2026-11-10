# WP-304 — Upstream-compatible session files

**Milestone:** M3 · **Size:** M · **Status:** open · **Depends on:** —

## Goal
pi-rust writes and reads session files in the **upstream format** (same
JSONL schema, same directory layout), so sessions are interchangeable with
pi (upstream) in both directions.

## Scope
- Port the session schema from upstream `core/session*` (entry types,
  ids, timestamps, usage fields, custom entries, tree/branch structure if
  present at the pinned commit).
- Replace the pi-rust v0 JSONL; migrate nothing (v0 files get a one-shot
  `--migrate-v0` flag or are left behind — decide during implementation).

## Acceptance
- Round-trip test: write session, read back, byte/semantic equivalence;
  golden-file test against a real upstream session; sidecar `appendEntry`
  still lands in the file (extension matrix re-run).

## Out of scope
- Session editing/branching UI (WP-305 is resume/list only).
