# WP-904 — Error UX & retry

**Milestone:** M9 · **Size:** M · **Status:** open · **Depends on:** M2

## Goal
Friendly, actionable errors (no raw panics to users): provider 401/429/5xx
mapped to guidance (wrong key / rate limit / retry), one automatic retry
with backoff on transient network errors only, TUI error banners.

## Acceptance
- Per-class error tests (mock server): 401 → key guidance, 429 → retry-
  after honored, 5xx ×2 → retry then fail cleanly; no key/URL-with-creds in
  any error text (redaction test).

## Out of scope
- Infinite retry queues.
