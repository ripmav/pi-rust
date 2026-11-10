# Milestones M1–M9

Ordering principle: make an existing pi user's setup keep working (M3) before
adding surface (M4/M5), build the TUI on working sessions/models (M6), finish
the extension contract (M7), ecosystem glue (M8), then harden and ship (M9).
M5 can start in parallel with M4 — it has no dependency on providers.

| # | Milestone | Goal | WPs | Status |
|---|-----------|------|-----|--------|
| M1 | Extension bridge (PoC) | original loader on a Rust core, NDJSON bridge, 6 unmodified extensions proven | — | ✅ done (PR #2) |
| M2 | Real model providers | openai-compat + anthropic, streaming, tool calls, env-only creds | — | ✅ done (PR #4) |
| M3 | `~/.pi` & session state | a user's existing config/sessions just work | 301–307 | done (PR #18, pending merge) |
| M4 | Model ecosystem | all provider families + runtime model switching + accounting | 401–408 | open |
| M5 | Built-in tool parity | read/write/edit/bash behave like upstream | 501–506 | open |
| M6 | Interactive TUI | ratatui REPL: chat, dialogs, editor, keybindings, themes, pickers | 601–610 | open |
| M7 | Extension completeness | the remaining contract rows: providers, MCP, compaction, shortcuts | 701–708 | open |
| M8 | Ecosystem | skills, RPC-mode parity, package evaluation | 801–804 | open |
| M9 | Hardening & release | CI, clippy/fmt, releases, error UX, perf, docs | 901–907 | open |

## Milestone entry/exit

- **Entry**: all dependency WPs done; upstream pin decision (keep or bump)
  recorded in the milestone doc.
- **Exit**: unmodified-extension matrix re-run green; `cargo test` +
  `node sidecar/smoke.mjs` green; README updated; milestone PR(s) merged.

## Dependency map

```
M1 ──► M2 ──┬─► M3 ──► M6 (sessions, models, dialogs in the TUI)
            ├─► M4 ──► M6 (model picker, usage display)
            └─► M5 ──► M6 (tool rendering)
M3+M4 ──► M7 (custom providers need registry; MCP needs config)
M6 ──► M7 (shortcuts/themes render in TUI)
M8 ──► M9 (release needs stable feature set)
```
