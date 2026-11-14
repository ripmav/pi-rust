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

## Evidence (delivered)

**Status: delivered 2026-11-14.**

- Bridge protocol v6 (lockstep): `protocolVersion: 6` in `src/cli.rs` init +
  sidecar reject; TUI `capabilities.ui` gains `"custom"`; smoke literals v6.
  Docs: `docs/bridge-protocol.md` § Custom pane (v6).
- Sidecar pane host (`sidecar/api.mjs` `createPaneHost`): the factory's real
  pi-tui component runs sidecar-side; the descriptor walk finds
  `SelectList` (items/cursor), `Input` (label/value/placeholder), `Text`
  (title + lines) via the component tree; out-of-subset components degrade
  to `render(width)` lines with `stripTerminalSequences`; a throwing factory
  resolves `undefined` with a sidecar log. Keys: `pane_key {id, key:{code,
  text?}}` → pi-tui input-string encoding → focused component (root
  `handleInput`, else first `capturesViewportInput` descendant — the
  SelectList case — else any `handleInput` descendant; the real TUI focuses
  one control, mirrored). `requestRender` (the only tui method both
  production callers use) + post-key recomputation push change-only
  `pane_update` (signature-deduped); `done(result)` resolves the promise and
  emits `pane_close`. `done` never crosses the wire.
- Core/TUI: `src/agent.rs` forwards `pane_*` to the TUI (headless never
  sees them — capability-gated on the sidecar); `src/tui/pane.rs` renders
  the descriptor (title/lines/list-cursor/input/blocked) in the transcript
  slot above the editor, below dialogs; `src/tui/terminal.rs` routes keys to
  the open pane before dialog/normal handling (the pane owns input, the
  editor is guarded) and sends `pane_key` fire-and-forget.
- E2E:
  - `tui_custom_pane_roundtrip` (PTY, `tests/tui.rs`): both WP-707
    production shapes — the mcp-manager menu (frame + SelectList; down +
    enter → "beta") and the progress pane (live `step 2/3` pane_update +
    esc → "cancelled"); fixture commands throw on wrong answers
    (`tests/fixtures/custom-panes/`).
  - `wp1006_headless_custom_is_noop` (`tests/extension_bridge.rs`): headless
    (ui: []) → factory never runs, resolves undefined, command ok, no
    pane_* events.
- Regression: dialog PTY (`tui_extension_dialog_roundtrip`) green; full
  suite green; `cargo fmt --check` clean; `cargo clippy --all-targets` 0
  warnings.
