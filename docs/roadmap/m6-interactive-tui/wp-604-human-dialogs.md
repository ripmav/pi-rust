# WP-604 — Human dialog answers (select/confirm/input/notify)

**Milestone:** M6 · **Size:** M · **Status:** open · **Depends on:** 601

## Goal
The bridge decision events (`select`, `confirm`, `input`, `editor`,
`notify`, `setStatus`, `setTitle`) get real human UI: the existing
headless default-answers path is replaced by rendered dialogs; answers flow
back over the unchanged bridge protocol.

## Acceptance
- E2E per dialog: extension asks → TUI renders → user answers → extension
  receives answer (fixture extension asserts); timeouts per upstream
  `timeoutMs` still apply; headless mode (no TTY) keeps default-answers
  behavior.

## Out of scope
- `ctx.ui.custom` (WP-707).
