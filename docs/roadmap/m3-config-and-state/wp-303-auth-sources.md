# WP-303 — API key resolution order

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** 302

## Goal
Keys resolve in the **verified upstream order** (`ai/src/auth/resolve.ts`,
docstring: "A stored credential owns the provider"): explicit override →
`auth.json` stored credential → ambient env var → clear "no credentials"
error naming the exact env var. (Original draft of this WP had env first;
corrected against upstream source during M3 execution.)

## Scope
- Port the upstream auth-storage read (file format from `auth-storage.ts`),
  read-only in v1. Stored `api_key` credentials with literal keys and
  `${ENV_VAR}` template references (credential `env` overlay shadows ambient
  env). **Deviation v0:** command-referenced keys are not executed — a typed
  error instead (no shell execution from `auth.json`).
- OAuth credentials in `auth.json` → typed "not supported in v0" error.
- Never print key values in logs/errors (security pin: test that error text
  contains no key material).

## Acceptance
- Resolution-order tests (env wins over stored; stored used when env absent);
  no-key error names the env var; redaction test on error output.

## Out of scope
- Storing new keys (interactive onboarding) — TUI milestone.
