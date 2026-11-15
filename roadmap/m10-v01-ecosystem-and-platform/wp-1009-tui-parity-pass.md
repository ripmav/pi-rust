# WP-1009 — TUI parity pass vs upstream p2

**Milestone:** M10 · **Size:** M · **Status:** in progress · **Depends on:** 1001

## Goal
Close the functional TUI gaps that opened when upstream moved past the
pin: the p2 work (light/dark theme cross-fade, app-wide appearance setting,
per-conversation-location MCP) plus the documented v0 TUI deltas, within the
functional-parity standard (pixel-perfect stays a non-goal).

## Scope
- Audit at the new pin (1001): diff the interactive surface — theme
  handling (upstream `theme-tokens.ts`/`oklab.ts` changes since the pin),
  appearance/light-dark handling, keybindings deltas, MCP per-location
  semantics vs the agent-dir `mcp.json` model.
- Implement the top functional gaps, prioritized: (1) light/dark theme
  handling if the TUI can't express it (theme system exists from WP-607 —
  extend, don't rebuild), (2) MCP location semantics where an unmodified
  extension depends on them, (3) keybinding parity deltas.
- TUI hardening sweep of the documented LOWs: WP-906 BashCapture leak on
  pane close (bounded; fix the drop), WP-606/708 logged-ignored list —
  re-evaluate each entry, implement what is cheap, re-document the rest.
- PTY matrix re-run (3 PTY tests + any new parity tests).

## Acceptance
- Per-item table in this file (gap, upstream ref, closed/documented,
  rationale) — the same evaluation-memo style as M7.
- Light/dark or appearance change (if implemented) demonstrable in the PTY
  harness (theme switch visible).
- PTY + full suite green; matrix green.

## Out of scope
- Pixel-perfect Ink/React parity (vision non-goal, unchanged).
- Per-location MCP as a product feature for core config (extension-facing
  semantics only).

## Audit — per-item table (2026-11-15, explore subagent, pin 17d6ca7)

| # | Gap | Upstream ref | pi-rust ref | Status | Rationale |
|---|---|---|---|---|---|
| 1 | Self-rendered huge tool result crash (`renderShell:"self"` V8 spread) | `5b5be2c`, `tool-execution.ts:240-256` | `src/tui/mod.rs:1237-1252` | already-safe | Rust renders tool results as transcript lines; no V8 spread; extension renderers are a documented v0 cut, not a crash path |
| 2 | `read` non-vision image warning kept out of model context | `4be4cb4`, `core/tools/read.ts:293` | `src/tools.rs:718-726` | **fix (T9b)** | pi-rust has no `imageUnsupportedByModel` detail / TUI-only warning at all; warning-in-context can't happen today, but the UX parity (placeholder + hint) is the gap |
| 3 | `system` theme (OKHSL/WCAG generated from terminal colors) | `theme/system-theme.ts`, `theme-controller.ts:173` | `src/tui/theme.rs` header | documented-out | no v0 equivalent (WP-607); needs OKHSL generator port — out of functional-parity core |
| 4 | Light/dark auto-pair following terminal appearance | `theme-controller.ts:200-253`, `settings-selector.ts:342-373` | `src/tui/theme.rs` (`resolve_theme_setting`) | documented-out | needs terminal color-scheme OSC query; crossterm has no query API in v0 |
| 5 | Runtime theme switch | `theme-controller.ts` `setThemeName` | `/theme <name>` + `set_theme` event + `pi.setTheme` | already-safe | implemented |
| 6 | Token grammar / 60 tokens / dark+light JSON / custom themes | `theme/theme.ts`, `light.json`, `dark.json`, `oklab.ts` | `src/tui/theme.rs` + `src/tui/themes/` | already-safe | verbatim port + golden vectors |
| 7 | Project `<cwd>/.pi/mcp.json` per-location MCP (+override merge, trust gate, `autoEnableCodemode`) | `extensions/mcp/config.ts:92-178` | `src/config.rs:85-100` (agent-dir only) | **fix (T9a)** | the only genuinely missing behavior an unmodified user/extension can trip; upstream built-in mcp extension is host-fed, so the loading side is pi-rust's |
| 8 | Keybinding deltas (linux non-WSL) | `packages/tui/src/keybindings.ts` | `src/tui/keybindings.rs` | already-safe | all 47 TUI + 43 APP actions match line-by-line; golden test asserts the set |
| 9 | Win32/WSL + darwin keybinding platform overrides | `core/keybindings.ts` `useWindowsKeybindings` | hardcoded linux table | documented-out | platform-override table is a v1 nicety; documented, not wired |
| 10 | Keybinding manager hot `reload()` / user-binding API | `core/keybindings.ts` | `KeybindingManager::load` (load-once) | documented-out | minor; no unmodified extension depends on it in v0 |
| 11 | WP-906 #5 BashCapture temp file leak on aborted run | `roadmap/m9-.../wp-906-bridge-security-audit.md` | `src/tools.rs` `BashCapture` | **fix (T9c)** | cheap Drop-based cleanup; closes the last M9 LOW with code |
| 12 | WP-906 #3 no sidecar→core line cap | same | `src/agent.rs` reader | re-evaluated: keep-documented | extensions already have full node access; cap is hygiene, not parity |
| 13 | Stale doc: "picker/theme actions resolve but are unwired" | `odd/tasks/m6-interactive-tui.md:123` | wired by WP-609/WP-607 | **doc fix (T9d)** | stale v0 note |

### Tasks
- T9a — project `.pi/mcp.json` loading (override merge + trust gate + autoEnableCodemode) + tests (wp702-style)
- T9b — `imageUnsupportedByModel` detail + TUI-only read warning + tests
- T9c — BashCapture Drop cleanup + regression test (aborted run leaves no temp file)
- T9d — this table + stale m6 doc line + PTY/full-suite matrix re-run
