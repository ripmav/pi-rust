# WP-701 — `registerProvider` invocation

**Milestone:** M7 · **Size:** L · **Status:** open · **Depends on:** 302, 406

## Goal
Extensions that call `pi.registerProvider(...)` get working custom model
providers: their stream function is invoked by the core for matching
model ids (sidecar-hosted provider callback over the bridge, or in-sidecar
streaming — decide by reading upstream's provider architecture at the
pinned commit).

## Scope (split into sub-WPs at start)
- 701a: bridge protocol extension for provider stream requests (version
  bump, spec section, lockstep sidecar).
- 701b: core dispatch: model id → registered provider, streaming plumbing
  into the agent loop.
- 701c: matrix: upstream provider-extension example runs unmodified.

## Acceptance
- A fixture extension registering a fake provider drives a full
  conversation (text + tool call) end-to-end; provider errors surface as
  model errors; unregistered id → normal registry fallback.

## Out of scope
- Provider UI (model picker shows registered providers — WP-609 follow-up).
