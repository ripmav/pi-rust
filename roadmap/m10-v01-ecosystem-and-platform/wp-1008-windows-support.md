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
