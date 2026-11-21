# WP-1005 — Llama native provider capture

**Milestone:** M10 · **Size:** M · **Status:** planned · **Depends on:** 1001

## Goal
Extend WP-701 to capture the **native Provider object form** of
`registerProvider` (used by the bundled `llama` extension): the sidecar
retains the object, its `streamSimple` drives the existing `provider_stream`
path, and its env-based auth (`LLAMA_BASE_URL`/`LLAMA_API_KEY`) resolves.

## Scope
- Sidecar: on `registerProvider(name, providerObject)` — detect the object
  form, capture `{name, models, streamSimple, baseUrl?, apiKey?}` (upstream
  Provider shape at the new pin), register into `ready.providers` with the
  captured models.
- Auth: `getProviderAuth`/env-based context for the provider (upstream
  semantics — no new credential kinds; env vars only, matching WP-701).
- `refreshModels` → catalog population path (the llama router advertises
  models; decide at explore time whether a static capture suffices).
- Fixture: a native-form provider extension (scripted streamSimple) drives a
  full turn end-to-end, mirroring the WP-701 acceptance on the object form.
- The unmodified `llama` extension loads and registers its provider
  (headless: against a fake router or a typed "router not running" error —
  no real llama.cpp in CI).

## Acceptance
- Native-form provider extension (unmodified `llama` or equivalent fixture)
  registers models in `ready.providers` and drives a scripted full turn
  (e2e).
- Env auth resolves; missing router → clean typed error.
- Legacy config form (WP-701) unchanged (regression e2e green).

## Out of scope
- `/llama` TUI manager + HuggingFace download flow (ui.custom WP-1006, no
  auth storage in bridge — WP-708 verdict stands unless 1006 changes it).
- Multi-process model management (llama.cpp runs externally, unchanged).

## Evidence (2026-11-14)

- **Native object form captured** (`sidecar/bridge.mjs`): `registerNativeProvider` (and the
  builtin factory API's `registerProvider` with the loader-style string/object dispatch)
  stores the Provider object sidecar-side — functions never cross the wire; only the model
  list crosses via `ready`/`providers_changed` (`providersPayload` now merges legacy + native
  entries). The core's per-turn routing is untouched: `provider_stream` / `provider_stream_event`
  are reused.
- **Native stream path** (`handleProviderStream`): full model re-fetched sidecar-side from
  `getModels()`; env-based auth resolved here with the upstream `AuthContext`
  (`{env: process.env, fileExists: false}`) — llama degrades to key `"local"`; resolved
  `apiKey`/`baseUrl`/`headers`/`env` merged into the stream options; the provider's own
  `stream`/`streamSimple` runs; the event-forwarding tail is shared with the legacy form.
  Unresolved auth → clean typed `provider stream error` ("provider <id> is not configured"),
  turn fails with exit 2 (ndjson), no crash.
- **Unmodified `llama` hosted**: dist factory imported directly
  (`dist/extensions/llama/index.js` — not re-exported by index.js); registers via
  `pi.registerProvider(providerObject)`; `/llama` TUI command registers (out of scope,
  WP-1006). **Catalog**: sidecar-side `refreshModels` before `ready` (env-only credential
  `LLAMA_BASE_URL`/`LLAMA_API_KEY`, `allowNetwork: true`, 5s timeout); the `publish` gate
  calls `args.update()` (that is what mutates the provider's model list) — without it the
  catalog stays empty.
- **Test matrix** (2026-11-14):
  - `wp1005_native_provider_drives_turn` — native-form fixture
    (`tests/fixtures/native-provider.ts`, test-only): scripted full turn; the resolved auth
    key provably reaches the stream options (`native stream, key=native-key-ok`); one
    `model_change` to the extension model.
  - `wp1005_native_provider_unconfigured_is_typed_error` — fixture
    (`tests/fixtures/native-unconfigured.ts`): auth unresolved → exit 2 +
    `agent run failed: provider fakeunconfigured is not configured`, scripted turn not leaked.
  - `wp1005_unmodified_llama_via_mock_router` — UNMODIFIED llama extension + mock router
    (`tests/fixtures/llama-router-mock.mjs`: `/models`, `/props`, `/v1/chat/completions`
    SSE): catalog populated before `ready`, model switch accepted, turn 2 streamed from the
    mock (`llama-mock: switch`), `model_change` to `llama.cpp/gemma-mock`.
  - Smoke 73/73 (incl. `llama.cpp` registered with empty catalog without a router).
  - Legacy config form regression: WP-701 e2e + smoke provider checks unchanged/green.
- **v0 non-goals (documented)**: `/llama` TUI manager + HF download (WP-1006), credential
  STORAGE (env only, WP-708 verdict), model persistence across runs (`stored: {models: []}`).

**Status: delivered (2026-11-14).**
