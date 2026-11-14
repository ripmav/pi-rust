# WP-1007 — OAuth credential support

**Milestone:** M10 · **Size:** M–L · **Status:** planned · **Depends on:** 1001

## Goal
Replace the typed "oauth — not supported in v0" error in `src/auth.rs` with
real oauth credential support per the upstream implementation: `auth.json`
credentials of type `oauth` are validated, used, and refreshed the way the
pinned upstream does.

## Scope
- Upstream spec (pinned commit): `auth-storage.ts` oauth shape
  (`{type:"oauth", refresh, ...}`), `model-runtime.ts` token
  use/refresh semantics, `cli/auth-login.ts` + `auth-terminal.ts` login
  flow (terminal-based; which providers offer it at the new pin — list in
  explore).
- Core: `resolve_api_key` accepts oauth credentials for providers that
  support them; token acquisition/refresh follows upstream semantics
  (sidecar-hosted where upstream does it in-process — the sidecar can run
  the upstream login flow unmodified, which is the compatibility-layer
  way).
- `pi-rust auth login <provider>` subcommand mirroring upstream's
  (terminal flow via the sidecar; auth.json written in the upstream shape).
- Key material never in errors/logs (WP-904 redaction rule extended).
- Non-oauth providers + api_key path unchanged (regression tests).

## Acceptance
- OAuth-capable provider (the set the pinned upstream supports): `auth
  login` flow produces a valid auth.json entry; a run uses it (fake/real
  endpoint test per provider); expired token refreshes per upstream
  semantics.
- Typed error gone for oauth credentials on supported providers; unsupported
  providers keep a precise typed error.
- No key/refresh material in any log or error (test).

## Out of scope
- Browser-based flows beyond what upstream's terminal flow needs.
- OAuth for providers upstream doesn't support at the pin.
- Windows console specifics (WP-1008).
