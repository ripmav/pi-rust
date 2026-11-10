# WP-303 — API key resolution order

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** 302

## Goal
Keys resolve in upstream order: env vars (existing `*_API_KEY`) → pi auth
storage (`~/.pi/agent` auth files) → clear "no credentials" error with the
exact env var name.

## Scope
- Port the upstream auth-storage read (file format from `auth-storage.ts`),
  read-only in v1.
- Never print key values in logs/errors (security pin: test that error text
  contains no key material).

## Acceptance
- Resolution-order tests (env wins over stored; stored used when env absent);
  no-key error names the env var; redaction test on error output.

## Out of scope
- Storing new keys (interactive onboarding) — TUI milestone.
