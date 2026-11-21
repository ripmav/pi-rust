# WP-1214 — Port `prompt-customizer`

**Milestone:** M12 · **Story:** E1S4 · **Size:** S · **Status:** open · **Depends on:** WP-1210

## Goal
The `systemPromptOptions.sections` reference port
([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).

## Scope
- `core-ext/prompt-customizer`: build `tool_guidance` from
  `systemPromptOptions` (selected tools + skills) on
  `before_agent_start`; set or delete `sections.tool_guidance` exactly like
  the original.

## Acceptance
- Same e2e scenario as the unmodified original: guidance lines present per
  active tools/skills (e.g. "Use `read` for file contents"); section absent
  when nothing applies.
