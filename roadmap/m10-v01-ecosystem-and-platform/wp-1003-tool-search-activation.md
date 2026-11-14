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

## Evidence (2026-11-14)

- **Protocol v5** (lockstep): `init` `protocolVersion` 4→5 in core (`src/cli.rs`) and
  sidecar reject (`sidecar/bridge.mjs`); smoke literals + `tests/extension_bridge.rs` +
  `tests/rpc.rs` + `docs/bridge-protocol.md` title/sections updated.
- **Sidecar (bridge.mjs)**:
  - `createBuiltinExtension(name)` helper; **built-in `tool-search` hosted** via
    `ca.createToolSearchExtension()` (dist export present since pin 17d6ca7) — upstream
    main.ts auto-loads it; pi-rust now does the same.
  - `ready.tools` entries carry `exposure?` / `defaultActive?`; `ready.activeTools: string[]`
    new (initial active set).
  - Upstream initial-activation rule implemented at registration
    (`isInitiallyActive`: direct/model-only active unless `defaultActive: false`;
    codemode/deferred inactive; hidden never).
  - `runtime.getAllTools` / `getActiveTools` / `setActiveTools` now wired (previously the
    built-in mcp api proxied to missing runtime methods — the mcp extension's
    `getAllTools()` ranker call would have thrown).
  - `applyActiveTools` = single source of truth for `setActiveTools` (user extensions via
    `createBridgeActions.onActiveToolsChanged`, built-ins via `runtime.setActiveTools`):
    unknown + hidden names ignored (upstream), then `tools_activated` on the wire.
  - `tools_changed` + `tools_activated` coalesced per tick (final state); **decision
    responses flush pending tool events before writing the response** (change-only emit
    via state signature) — deterministic ordering with the core's drain barrier.
  - `createBridgeActions` gains `onActiveToolsChanged` (api.mjs `setActiveTools` delegates).
- **Core (agent.rs / cli.rs)**:
  - `Core.ext_active_tools` (ready.activeTools / tools_activated); `tools_changed` prunes
    active entries for vanished tools; `merged_tool_defs` filters ext tools to the active
    set (model declaration = registered ∩ active, upstream `types.ts:535`); execution
    routing unchanged (registered name → execute).
  - `event_task` `tokio::select!` now `biased;` (events branch first) — the WP-702
    `sync_tool_set` barrier is deterministic: a queued `tools_activated` written before a
    decision response is always applied before the agent loop continues. (Root cause of a
    full-suite flake found in the first green run: setImmediate flush raced the barrier
    under load.)
- **Test matrix** (all green, 2026-11-14, full suite + 71 smoke):
  - `tests/extension_bridge.rs::wp1003_deferred_tool_activated_mid_run` — e2e: fixture
    `tests/fixtures/tool-activation.ts` (test-only, not an unmodified upstream extension):
    `deferred_echo` (exposure deferred, defaultActive false) absent from turn 1's
    `before_agent_start.selectedTools`, present after `find_deferred` runs
    `pi.setActiveTools`, then executes (`deferred-echo:x`); `[core] tools_activated` on
    corelog.
  - `sidecar/smoke.mjs` — ready carries `activeTools`; `tool_search` registered inactive
    (model-only, defaultActive false, not active); hello active; extension counts 9 (main)
    / 3 (gitlab-duo) with the new built-in.
  - Full regression: 271 in-crate, 39 e2e, 4 rpc, 3 PTY, 3 skills, 71 smoke — 0 failed;
    fmt + clippy clean.
- **Docs**: `docs/bridge-protocol.md` new "Active tool set (v5)" section;
  `sidecar/README.md` handshake line 4→5 + tool-search mention.

**Status: delivered (2026-11-14).**
