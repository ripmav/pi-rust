# WP-1004 — Codemode hosting (factory + model-call bridge)

**Milestone:** M10 · **Size:** M · **Status:** planned · **Depends on:** 1001, 1003

## Goal
Host the unmodified `codemode` bundled extension: its tool works end-to-end
(QuickJS script → nested `ctx.executeTool`), and scripts' `modelRegistry`
model calls resolve instead of throwing TypeError.

## Scope
- Sidecar: host the `codemode` factory (WP-708 load check passes; the
  missing pieces are (a) active-set visibility — solved by 1003, (b)
  `ctx.modelRegistry` model calls, (c) `constrainedSampling` grammar
  handling).
- Model-call bridge: `getModelsOfType`/`classify`/`generateImages` on the
  `modelRegistry` stub → core request (which providers/models are reachable
  from the script: decide against the upstream types at the new pin).
  `generateImages`: evaluate feasibility at explore time — if it needs
  multipart image pipelines the core lacks, document as a typed sub-gap.
- `constrainedSampling`: core-side grammar handling for model calls that
  carry one (upstream semantics from the pinned commit).
- Acceptance fixture: a codemode script that lists models, classifies, and
  nested-executes a tool, asserting results.

## Acceptance
- Unmodified codemode loads; its tool activates (1003 path) and executes a
  script with nested tool execution (e2e).
- `getModelsOfType`/`classify` round-trip verified; `generateImages` either
  works or is a documented typed sub-gap with rationale.
- Matrix + smoke green.

## Out of scope
- Codemode UI renderers (logged-ignored list from WP-606/708, stays).
- Windows QuickJS-wasm specifics (WP-1008).

## Evidence (2026-11-14)

- **Hosting**: `loadBuiltinCodemodeExtension(runtime)` runs
  `ca.createCodemodeExtension({ models: false })` (dist export verified at pin 17d6ca7)
  against the same minimal factory API as mcp/tool-search. Loaded AFTER `bindCore` —
  the factory needs `runtime.appendEntry`/`runtime.getSettings`, which `bindCore` wires
  (runner.ts:431/437); both added to the factory API as runtime delegates.
- **`models: false`** (documented v0 cut): the `models.*` namespace (classify /
  generateImages) is a direct model-API call the v0 bridge cannot serve — needs a new
  wire event; follow-up. With `models: false` the namespace is never declared, so no
  code path in the sandbox reaches it.
- **Registration**: `codemode` registers inactive (exposure model-only, defaultActive
  false) → activation via the WP-1003 v5 active-set path. No protocol change (v5 stays).
- **Sandbox**: QuickJS wasm via `quickjs-wasi` + `@earendil-works/pi-codemode` (both in
  the pinned upstream `node_modules`; resolved from the dist file's own path — no
  sidecar action). Nested tool calls go through the EXISTING `tool_call_request` wire
  event (contextActions.executeTool) — zero new wire events; `ctx.tools` = extension
  tools (getCallableTools), `ctx.sessionManager.getBranch()` from the sidecar session
  view.
- **Test matrix** (2026-11-14):
  - `tests/extension_bridge.rs::wp1004_codemode_runs_script_and_nested_tool` — e2e:
    fixture `tests/fixtures/codemode-nested.ts` (test-only); model calls `codemode`
    with a script that awaits `tools.echo_param({text:"x"})` and computes `1+1`;
    asserts exactly one nested `tool_call` decision for `echo_param` and the codemode
    result carrying `echo-param:x` + `1+1=2` (sandbox ran).
  - `sidecar/smoke.mjs` — codemode registered inactive (model-only, defaultActive
    false, not in activeTools); extension counts 10 (main) / 4 (gitlab-duo).
  - Full regression + fmt/clippy: green (see feature doc L7).
- **v0 non-goals (documented)**: `models.*` (new wire event), `prepareLoadout`
  ("Nested tools:" declaration guidance — the model just sees the plain description),
  `reachesTools`/`constrainedSampling` (not serialized over the bridge), abort-signal
  propagation into the sandbox (the `timeout_ms` option still applies).

**Status: delivered (2026-11-14).**
