# WP-1008 — Windows support

**Milestone:** M10 · **Size:** M · **Status:** planned · **Depends on:** 1002

## Goal
Lift the "Windows until CI proves out macOS/Linux" non-goal (vision doc):
macOS/Linux are proven (M9), so `x86_64-pc-windows-msvc` joins as a fifth
release target and CI job.

## Scope
- Cross-build `x86_64-pc-windows-msvc` in `release.yml` (runner
  strategy: `cross`/`cargo-zig` or a windows runner — decide at explore;
  keep the existing four targets byte-identical).
- `install.sh` → Windows: keep `install.sh` POSIX-only; add
  `install.ps1` (upstream-parity check: what upstream ships) or document
  manual install (decide at explore).
- TUI on Windows: crossterm backend differences (alt-screen, key
  encoding, terminal title) — PTY harness is unavailable on Windows;
  coverage = headless e2e matrix + release smoke (`--provider fake`
  one-shot) + a manual TUI smoke checklist in the release notes.
- Node sidecar on Windows: path handling (already JS), spawn behavior
  (node.exe), no new dependencies.
- CI: test job on `windows-latest` for the headless suite (no PTY, no
  bash-isms; the bash tool is POSIX — its e2e expectations already degrade
  to typed "not available"? verify and fix where needed).

## Acceptance
- `pi-rust.exe` target built in CI and published with the next release;
  smoke (one-shot fake turn) passes on `windows-latest`.
- Headless e2e suite green on `windows-latest`.
- Install path (ps1 or documented) verified end-to-end once.

## Out of scope
- TUI PTY-level automation on Windows (no harness; manual checklist only).
- ARM64 Windows (revisit with demand).

## Manual TUI smoke checklist (Windows)

No PTY automation on Windows (WP out-of-scope); run once per release
candidate from a Windows terminal (PowerShell or Git Bash):

1. `pi-rust.exe --provider fake --prompt hi` prints `echo: hi` (headless,
   covered by the release smoke in CI).
2. `pi-rust.exe` (no one-shot flags) on a TTY: TUI opens, alt-screen,
   type a prompt, ctrl+c / `exit` closes it without crashing the
   terminal (crossterm alt-screen restore).
3. `/theme` and `/model` dialogs render and esc closes them.
4. An extension dialog (select/confirm/input) opens from a scenario run
   and responds to keys.
5. Windows console: no garbled ANSI (the TUI emits ANSI via crossterm;
   Windows Terminal + VS Code terminal handle it — classic
   conhost pre-Win10 is not a supported target).

## Evidence (2026-11-14)
