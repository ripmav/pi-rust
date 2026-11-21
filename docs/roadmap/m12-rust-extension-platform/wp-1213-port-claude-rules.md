# WP-1213 — Port `claude-rules`

**Milestone:** M12 · **Story:** E1S4 · **Size:** S · **Status:** open · **Depends on:** WP-1210

## Goal
The file-I/O + prompt-injection reference port
([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).

## Scope
- `core-ext/claude-rules`: recursive `.md` walk of `<cwd>/.claude/rules` on
  `session_start` (+ `ui.notify`), state carried to `before_agent_start`,
  same `## Project Rules` system-prompt injection.

## Acceptance
- Same e2e scenario as the unmodified original: rule files listed in the
  system prompt (`## Project Rules` + the file list); notify when rules
  found; no injection when the directory is absent.
