# WP-602 — Markdown rendering

**Milestone:** M6 · **Size:** M · **Status:** open · **Depends on:** 601

## Goal
Assistant/user messages render as markdown (headings, lists, code blocks
with language label, inline code, bold/italic, links as text) per upstream
visual contract.

## Acceptance
- Snapshot tests per construct (ratatui buffer snapshots); long code block
  scrolls within pane; CJK/emoji width test.

## Out of scope
- Syntax highlighting (evaluate later), math.
