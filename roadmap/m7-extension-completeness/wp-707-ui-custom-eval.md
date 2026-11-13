# WP-707 — Evaluate `ctx.ui.custom`

**Milestone:** M7 · **Size:** S · **Status:** done · **Depends on:** 604

## Goal
Investigation + decision: what is `ctx.ui.custom` at the pinned commit, who
uses it, and is a ratatui equivalent feasible (generic overlay pane?) —
write a decision memo (this file updated) with scope or non-goal.

## Acceptance
- Memo in-file: upstream usage examples, feasibility verdict, either a
  follow-up WP or a documented non-goal with rationale. No code.

## Out of scope
- Implementation (separate WP if approved).

## Decision memo (2026-11-12, upstream pinned 085c84e)

### What `ui.custom` is
`ExtensionUIContext.custom<T>(factory, options?) => Promise<T>` (`core/extensions/types.ts:228-262`): the factory builds a **live pi-tui component** — `render(width) -> ANSI lines`, raw `handleInput(data)` (the extension parses keys itself via `matchesKey`), optional `handleMouse`/focus/`invalidate()`, closed by `done(result)` which resolves the promise. Options: overlay vs inline (editor-slot swap), `blocked` message. Wrapped in `withUIPrompt("custom")` (fires `ui_prompt_start/end`).

### Who uses it at the pinned commit
- **Bundled (production): exactly 2 of 4** — `mcp/ui.ts:257` (the `/mcp` manager: menus that rebuild on subscription, `SelectList`, an `Input` for pasting the OAuth redirect URL) and `llama/ui.ts:512` (model view with live progress + cancel key). `codemode` and `tool-search` do **not** use it.
- **Examples**: ~15 (question/questionnaire/todo/handoff/interactive-shell + the games: snake, tic-tac-toe, space-invaders, doom-overlay, overlay-qa-tests). These compose the **full component model** (component trees, focus, cursors, mouse) — not expressible as data.
- Key fact: every bundled production usage needs only a **bounded subset: title + styled lines + list-with-cursor + one optional input + raw keys + subscription rebuilds**. The games are the rabbit hole, not the product surface.

### pi-rust state
The sidecar's `custom` is a no-op returning `undefined` (`sidecar/api.mjs:95`) — **identical to upstream's own RPC mode** (`rpc-mode.ts:233`). Headless runs never reach it. The TUI's dialog system (WP-604: select/confirm/input) is the one-shot request/response overlay mechanism; a generic `custom` pane would need a new message family (open/update/key/close, per-key round-trips, streaming content pushes) — it is not the dialog channel.

### Feasibility
- **(a) typed no-op (status quo)**: hours; graceful — callers receive `undefined` exactly like upstream RPC mode, and the real callers guard for it (examples check `ctx.mode`/result).
- **(b) descriptor overlay pane**: new bridge pane messages + sidecar handler + TuiApp pane state + key round-trip + render slot; **days-scale WP**. Covers the mcp-manager and llama-progress shapes (and most tool question UIs) — the full component model still does not map to JSON.
- **(c) full component bridge**: re-implementing pi-tui in Rust — not feasible in v0.

### Decision: **documented v0 non-goal (tier a); tier (b) is the scoped follow-up candidate**
Rationale:
1. Headless-first v0: most runs never construct a TUI, and the capability-gated no-op already matches upstream RPC behavior — nothing breaks, only the interactive mcp-manager/llama UX and the community game examples degrade (gracefully).
2. The blocking-UI needs (pickers, confirmations, inputs) are served by the WP-604 dialog subset; the agent loop has no dependency on `custom`.
3. Enabling tier (b) widens the bridge protocol (per-key latency + streaming pane updates) for a surface whose production users are the two bundled extensions — and `mcp` is already bridged (WP-702/703) without its manager UI (server lifecycle + tools work; `/mcp` management stays manual via `mcp.json`/`registerMcpServer`).
4. **Follow-up WP (if approved, post-v0)**: "Descriptor overlay pane" — JSON descriptor (title, styled lines, list+cursor, optional input) + `pane_open/pane_update/pane_key/pane_close` bridge family + TuiApp overlay slot; acceptance = the mcp manager menu and a progress pane round-trip in the PTY harness. Explicitly out: mouse, focus trees, component composition, animations.
