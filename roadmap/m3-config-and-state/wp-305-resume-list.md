# WP-305 — Session list, `--resume`, fork

**Milestone:** M3 · **Size:** S · **Status:** open · **Depends on:** 304

## Goal
`pi-rust --list-sessions`, `--resume <id|path>` (continue a session with its
full transcript as conversation), `--fork <id>` (new session from an old one).

## Acceptance
- Resume test: scripted scenario continues a pre-seeded session; transcript
  contains history; fork creates a new file sharing history prefix.

## Out of scope
- Interactive session picker (TUI, WP-609).
