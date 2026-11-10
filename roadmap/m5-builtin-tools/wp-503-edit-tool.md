# WP-503 — `edit` tool

**Milestone:** M5 · **Size:** M · **Status:** open · **Depends on:** —

## Goal
Built-in `edit` with upstream exact-replacement semantics (unique match,
multiple non-overlapping edits, error on ambiguity/missing).

## Acceptance
- Table tests per upstream behavior doc: unique, multi-edit, no-match,
  ambiguous, overlapping → exact error strings; applied diff verified.

## Out of scope
- Fuzzy matching.
