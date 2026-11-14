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
