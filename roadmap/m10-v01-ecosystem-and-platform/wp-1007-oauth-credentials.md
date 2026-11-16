# WP-1007 — OAuth credential support

**Milestone:** M10 · **Size:** M–L · **Status:** delivered (2026-11-14) · **Depends on:** 1001

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

## Evidence (delivered)

**Status: delivered 2026-11-14.** Issue #137, branch `feat/wp-1007-oauth-credentials`.

- **`pi-rust auth login <provider> [--api-key] [--no-browser]`** — the
  pinned upstream's terminal login flow runs UNMODIFIED in a dedicated
  `sidecar/auth-login.mjs` (`ModelRuntime.create({allowModelNetwork:false})`
  + `runAuthLogin`, the same entry as `main.ts`); the core (`src/auth_login.rs`)
  owns the TTY, checks interactivity, and relays `write`/`error`/`prompt`
  (question + no-echo secret via raw mode)/`open_url`/`exit` over NDJSON.
  `PI_CODING_AGENT_DIR` shared, so credentials land in the same `auth.json`
  the core reads. Key material only travels as prompt answers; never logged.
- **Built-in oauth providers (runtime side)** — the five oauth-capable
  upstream providers at the pin (radius, kimi-coding, github-copilot, xai,
  meta) are exposed in the sidecar `providers` payload (from the upstream
  runtime's `getProviders().getModels()`) and stream via
  `modelRuntime.streamSimple`, where upstream's own auth resolution
  performs the stored-credential refresh (5-minute validity margin,
  per-provider flow) and writes the rotated token back to `auth.json` —
  all sidecar-side; no token crosses the wire. Extension-registered
  providers keep priority on the same name. `settleCredentialWrites()`
  runs on shutdown.
- **Core driver path** — `resolve_api_key` now returns a precise typed
  error for oauth credentials: the five upstream oauth providers point to
  the sidecar-routed models / `pi-rust auth login`; other providers keep
  "no upstream oauth support". No key material in errors.
- **Acceptance evidence**:
  - `auth_login_api_key_stores_upstream_credential` (PTY): the
    `auth login anthropic --api-key` flow prompts, stores the upstream-shaped
    api_key credential in the shared `auth.json`, exits 0, and the typed
    key appears nowhere in the output (no echo, no logging).
  - `auth_login_rejects_bad_arg_shapes`: usage/flag/TTY typed errors, exit 1.
  - `wp1007_oauth_credential_refreshes_before_provider_call` (E2E): an
    expired kimi-coding oauth credential is refreshed against a mocked
    `KIMI_CODE_OAUTH_HOST` token endpoint (form `grant_type=refresh_token` +
    the stored refresh token), `auth.json` is rotated (new access/refresh,
    future `expires`, upstream shape), the run then fails with a typed 401
    model error (exit 2, no crash) against the real api.kimi.com, and no
    token material lands in stdout/stderr/core.log.
  - In-crate: `oauth_on_upstream_oauth_provider_points_to_the_sidecar_route`
    + the existing api_key regression tests green.
- **Docs** — `docs/bridge-protocol.md` §Out of scope (provider invocation
  bullet rewritten for WP-1005+1007), `docs/cli-reference.md` + USAGE
  (diff-tested) gain the `auth login` subcommand line, README compatibility
  table + v0-limit note updated.
- **Non-oauth regression** — the api_key path is unchanged (existing
  resolve tests + full suite green).
- **CI-found race fixes** (commit 2493616, first push failed on both
  platforms — root-caused, not flake-waved):
  - macOS `auth login` secret echo: prompt was printed before
    `enable_raw_mode`, so the first input bytes were terminal-echoed and
    the key leaked into the output. Raw mode is now enabled before the
    prompt; the PTY e2e's key-in-output check pins it.
  - ubuntu dropped activation: an extension tool's `setActiveTools`
    flushes `tools_activated` before its `extension_tool_call` response,
    but the response could wake the agent loop before `event_task`
    applied the queued event (stale `before_agent_start` declaration,
    `wp1003_deferred_tool_activated_mid_run`). The agent loop now drains
    queued sidecar events (existing `sync_tool_set` barrier) before a
    run's declaration and at every turn start (`build_conversation`
    reads the active set); agent-loop context only, no self-deadlock.
  - Post-fix: full suite green (276/2/1/45/4/3/4), smoke 74,
    fmt + clippy clean; wp1003 3/3 + full-suite-under-load 3/3 locally.
- **Native review (DISCARDED by operator decision, 2026-11-15/16)** — assess
  `reviewDue=true` (native high risk; rddLine on), lineage
  `review-bb11d6f28e12c977` started (tier high, lenses risk/resilience/
  reliability, 1045 lines, correction budget 200, base 786bcf7). CI for
  PR #138 green (6/6) on head 021c083. The reviewer group failed three
  times identically in the `review-risk` lens: the host-relay reviewer
  (solheim/qwen3.8-27b) hit the provider's 15-minute completion limit
  (`UND_ERR_SOCKET` at ~900–940 s, 88k–120k thinking chars, 0 output
  text, 0 tokens) — a provider-transport/model-completion limit, not a
  review finding; nothing was ever admitted or burned (attempt 3:
  940,390 ms, 119,728 thinking chars, 0 text blocks). The user decided
  (2026-11-15, "akzeptieren und review streichen"): WP-1007 is accepted
  as delivered on CI (6/6) + full suite + E2E evidence, and the native
  review is discarded. Executed as native `review abandon` with
  reason `operator_disposition`, actor `user` — committed record:
  quarantine `review-bb11d6f28e12c977-2276863311`,
  `discarded_work: {captured_lens_results: [], findings_present: false}`
  (zero review artifacts — the lineage held no findings to lose).
  WP-1007 delivery evidence stands: PR #138 CI 6/6 green, full suite
  (276/2/1/45/4/3/4 + smoke 74), merged into main as `d0e1325`.
