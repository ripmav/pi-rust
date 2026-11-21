# WP-801 — Skills system

**Milestone:** M8 · **Size:** M · **Status:** open · **Depends on:** 301

## Goal
Upstream skills: discovery (agent/project dirs), SKILL.md frontmatter
parsing, availability to the model (system-prompt injection per upstream
mechanism), headless + TUI command.

## Acceptance
- Fixture skill discovered; frontmatter validated (name/description);
  model prompt contains the skill block per upstream format (golden);
  invalid skill skipped with warning.

## Out of scope
- Skill authoring tooling.
