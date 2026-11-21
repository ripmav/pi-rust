# WP-605 — Editor pane (input)

**Milestone:** M6 · **Size:** M · **Status:** open · **Depends on:** 601

## Goal
Multiline input: paste (bracketed), history (up/down), word-wise editing,
`editor` dialog support (WP-604) reusing the pane, submit on Enter /
shift+enter for newline (per upstream keybindings).

## Acceptance
- Key-sequence tests via crossterm simulation: paste 50-line block (no
  corruption), history recall, multiline submit; input dialog reuse test.

## Out of scope
- vim/emacs modes.
