# WP-1006 — `ctx.ui.custom` tier (b): descriptor overlay pane

**Milestone:** M10 · **Size:** L · **Status:** planned · **Depends on:** 1001

## Goal
Implement the scoped follow-up from the WP-707 decision memo: a JSON
descriptor overlay pane so `ctx.ui.custom` stops being a no-op for the
bounded production shapes (mcp manager menus, llama progress, tool question
UIs) while the full component model stays a documented non-goal.

## Scope
- Bridge (lockstep version bump + docs section): `pane_open` /
  `pane_update` / `pane_key` / `pane_close` family (sidecar↔core),
  descriptor = `{title?, lines: [{text, style?}], list?: {items, cursor},
  input?: {label, value?}, blocked?}` — the exact subset WP-707 identified.
- Sidecar: `ctx.ui.custom(factory, options)` hosts the descriptor flow —
  the factory's component calls translate to descriptor updates where they
  fit the subset; `done(result)` resolves the promise with the result.
  Components outside the subset → typed graceful degradation (documented,
  no crash).
- Core/TUI: overlay slot in the TuiApp (above the editor, below dialogs),
  key round-trip to the sidecar, `blocked` message rendering, close on
  `pane_close`; focus/invalidate semantics per the WP-707 memo.
- PTY acceptance: the mcp manager menu (list + cursor + one key) and a
  progress pane (live updates + cancel key) round-trip in the PTY harness —
  the exact acceptance the WP-707 memo names.

## Acceptance
- The two WP-707 production shapes (mcp-manager menu, progress pane)
  render, receive keys, update live, and resolve via `done(result)` (PTY
  e2e + smoke).
- Out-of-subset components degrade with a documented typed message
  (fixture test), never a crash.
- Existing dialogs (WP-604) unaffected (regression PTY green).

## Out of scope
- Mouse, focus trees, component composition, animations (WP-707 verdict).
- Games-style examples (snake/doom) — documented non-goal, unchanged.
- `ui_prompt_start/end` bridging (stays logged; revisit with real users).
