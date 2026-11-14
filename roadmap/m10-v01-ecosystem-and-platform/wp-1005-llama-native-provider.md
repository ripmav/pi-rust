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
