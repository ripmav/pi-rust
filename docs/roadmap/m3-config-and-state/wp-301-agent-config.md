# WP-301 — Read agent config from `~/.pi/agent`

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** —

## Goal
The core loads the same agent config a pi user already has (`settings`
files, env expansion), so existing behavior (default model, cwd defaults)
is inherited.

## Scope
- Locate `~/.pi/agent` (or `--agent-dir`) config files; read the settings
  surface the core actually needs (default model, default thinking level,
  bash timeout, …) — start from what upstream `config.ts` + `defaults.ts`
  read.
- Unknown/extra keys: ignore (forward compat).

## Acceptance
- With a populated `~/.pi/agent`, `pi-rust --prompt …` uses the configured
  defaults; a unit test per read key; malformed config → clear error (exit 1).

## Out of scope
- Writing config; config hot-reload; any key that only the TUI consumes
  (WP-607 handles theme/keybinding loading).
