# WP-1003 — tool-search activation (active-set wire event)

**Milestone:** M10 · **Size:** S · **Status:** planned · **Depends on:** 1001

## Goal
Make `setActiveTools` observable on the wire: host the tool-search factory
and emit an active-set change event, so default-inactive tools become
activatable by the model. Shared enabler for WP-1004 (WP-708 memo: "cheapest
of the four").

## Scope
- Sidecar: host the `tool-search` inline factory (WP-708 pattern, dist
  factory export).
- Bridge (lockstep version bump + docs section): sidecar→core event
  `tools_activated {active: [...]}` emitted after `setActiveTools`
  (`tools_changed`-style coalescing, WP-702 pattern); tool payload carries
  `exposure` (currently dropped) so the core can declare deferred tools.
- Core: apply the activation set to the model-visible tool list; the
  `tool_search` tool itself stays model-visible per upstream
  `reachesTools: all`.
- Fixture extension registering a default-inactive tool + activating it via
  `setActiveTools` → e2e proves the tool becomes callable in the same run.

## Acceptance
- Unmodified `tool-search` from `dist/extensions` loads and its tool
  activates a deferred fixture tool mid-run (e2e + smoke).
- `exposure` present in the tool payload (smoke check).
- No regression for always-active tools (existing matrix green).

## Out of scope
- Codemode-specific model calls (WP-1004).
- Codemode exposure semantics beyond carrying the field (WP-1004).
