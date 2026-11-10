# WP-704 — Compaction (auto + events)

**Milestone:** M7 · **Size:** L · **Status:** open · **Depends on:** 304, 407

## Goal
Context compaction per upstream `core/compaction`: trigger at context
threshold, summarize via the active model, rewrite the conversation, emit
the upstream compaction events to extensions.

## Scope (split at start)
- 704a: threshold/trigger + event emission (headless visible).
- 704b: summarization prompt + conversation rewrite (golden tests).
- 704c: session file integration (compaction marker entries, upstream
  format).

## Acceptance
- Scripted long session triggers compaction; transcript continues
  coherently (fake model assertions on what it sees); compaction events
  received by a fixture extension; session file stays upstream-compatible.

## Out of scope
- Micro-compaction variants beyond upstream v1.
