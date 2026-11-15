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

- PR #140 (stacked on the WP-1007 head), issue #139. First
  windows-latest run failed at compile (16 errors, three unix-only
  spots): `ExitStatusExt` import + `128+signal` fallback in
  `src/tools.rs` (unix-gated; windows children always carry a code),
  and the `tokio::signal::unix` SIGTERM branch of the rpc loop in
  `src/cli.rs` (windows equivalent: console ctrl-c, same 143 path).
  Fix commit `c7e938b`; local linux suite re-green after the fix
  (276/2/1/45/4/3/4, smoke 74, fmt + clippy clean). Second run compiled
  but 13 unit tests failed: the runner's PATH `bash` is the WSL launcher
  (exit 1, no distro) — the bash tool now resolves Git Bash
  (`C:\Program Files\Git\bin\bash.exe`) first on windows (typed
  "no POSIX shell" error otherwise); the 137-signal golden and the rpc
  SIGTERM->143 test are unix-gated; the two skill goldens now build
  expectations from the loaded skill's own paths (windows string-join vs
  dir-walk separator serialization). Fix commit `4989344`; linux suite
  re-green. Third run: 271/272 — the write-to-directory golden
  (`tools::write_to_directory_eisdir_upstream_error`) because libuv maps a
  write-open on a directory to `EISDIR` on unix but `EACCES` on windows
  (both correct rejections); the golden now accepts the platform errno. Fix
  commit `ace1897`; linux suite re-green. Fourth run: all 42 e2e tests
  failed with `ERR_UNSUPPORTED_ESM_URL_SCHEME` — Node's ESM loader rejects
  `import("D:\\...\\index.mjs")` because the drive letter parses as an
  unknown URL protocol; the sidecar's dynamic imports (`bridge.mjs`,
  `auth-login.mjs`) now go through `pathToFileURL(p).href` (the CLI entry
  points stay plain paths — Node resolves the entry itself, and
  `node file://...` is not a valid entry on any platform). Fix commit
  `e3af7a3`; fifth run: windows e2e 42/45 — the last three were test-side
  path serialization: the wp702 `mcp.json` fixtures embedded raw Windows
  paths (invalid JSON escapes) and the upstream-SessionManager cross-check
  embedded a native path in a JS string + a `file://`-prefixed specifier
  (both invalid on windows); both now travel via `json!`/env +
  `pathToFileURL`. Fix commit `a7e0e01` (with the auth-login PTY harness
  diagnostics for the concurrent ubuntu round-6 failure). Local linux suite
  green throughout each round (276/2/1/45/4/3/4, fmt + clippy clean);
  the windows gate is the CI `windows-latest` job itself (local
  cross-compilation of `aws-lc-sys` from linux to msvc is impossible).
  Sixth run: e2e 45/45; the two remaining failures were the rpc tool-event
  tests — the bash tool child hung after `tool_execution_start` (Git Bash
  on windows-latest, 30s timeout). Root cause: pi-rust inherited stdin
  into the shell child, and in rpc mode that is the RPC client pipe;
  upstream parity is `stdio:["ignore","pipe","pipe"]` + `windowsHide:true`
  (`core/tools/bash.ts`). Fix commit `c8e7730`: `Stdio::null()` for the
  shell child (also stops a stdin-reading tool command from swallowing
  RPC protocol bytes on any platform) + `CREATE_NO_WINDOW` on windows.
  Local linux suite re-green.
  Seventh run: rpc 3/3 — but `tests/skills.rs` failed 0/3 for the first
  time because it ran for the first time: cargo test is fail-fast per
  test binary, and `rpc` (alphabetically earlier) had masked `skills` and
  `tui` in every prior windows run. The skills integration tests built
  path expectations with string joins (mixed separators on windows) vs
  the core's native backslashes. Per-component joins + a separator-free
  warning check fix them. Fix commit `5ef5289`; linux suite re-green.
  Eighth run (2026-11-15): **all 7 checks green**, including
  `windows-latest` (lib 272, auth 2, cli_help 1, e2e 45, rpc 3, skills 3,
  tui 4 self-skipped; ubuntu + macos suites green; clippy/rustfmt/bench/
  fresh-clone green). PR #140 is delivery-ready.
