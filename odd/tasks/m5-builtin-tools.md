# M5 — Builtin Tools

Milestone 5 of the pi-rust port: the built-in tool suite reaches upstream
semantics (read/write/edit/bash + permissions + registry parity), each WP as
one PR in one GitHub stack (user instruction 2026-11-11).

## Specs

- S1 (user, verbatim): "Arbeite autonom die Roadmap ab. Jeder Milestone wird ein Stack, wenn möglich mit main als base, und jedes WP wird ein PR im Stack. Jedes WP soll in möglichst kleine Arbeitspakete aufgeteilt werden welche dann, abgearbeitet, als commit im jeweiligen Branch/PR commited und gepushed werden."
- S2 (WP-501): Built-in `read` with upstream semantics: file contents, `offset`/`limit`, 50KB/2000-line truncation contract, image files detected (WP-408 consumes). Golden tests: exact stdout for truncated file, binary detection, missing file error text matches upstream.
- S3 (WP-502): Built-in `write`: full-file overwrite/create with parent-dir creation, upstream result message format. Unit tests: create, overwrite, nested dirs; result summary format matches upstream string; permission error mapped.
- S4 (WP-503): Built-in `edit` with upstream exact-replacement semantics (unique match, multiple non-overlapping edits, error on ambiguity/missing). Table tests: unique, multi-edit, no-match, ambiguous, overlapping → exact error strings; applied diff verified.
- S5 (WP-504): Built-in `bash` reaches upstream semantics: timeout (config-driven, WP-301), output truncation (last N lines/bytes), exit-code propagation, `PI_*` env for the child, cwd = project. Acceptance: timeout kill test (1s limit), truncation golden test, non-zero exit reported with stderr, long-output memory bounded (10MB input test).
- S6 (WP-505): Upstream permission behavior: default-allow with extension gate hooks (the existing `permission-gate.ts` pattern is the contract). Acceptance: `permission-gate.ts` unmodified still blocks (matrix re-run); headless mode answers per upstream defaults. TUI confirm path is WP-604 (M6), out of scope here.
- S7 (WP-506): Core tool registry exposes upstream names/descriptions/JSON schemas exactly (what the model and extensions see); `ready.tools` passthrough + built-ins merged with upstream precedence (extension shadowing rules). Acceptance: schema-diff test against upstream-generated schemas (golden files from upstream at the pinned commit); shadowing test.

## House rules for M5 (S1-derived)

- One GitHub stack for M5, first PR based on `main`, each subsequent WP PR
  based on the previous PR's head; every PR registered via the stacks API
  (`POST /repos/ripmav/pi-rust/stacks/{n}/add`).
- Exactly one PR per WP; every WP split into small work packages, each
  committed AND pushed as it completes (work-unit commits, ~≤400 authored
  lines).
- Branch naming `feat/<wp-slug>`; each PR links its WP issue + one
  `type:*` label; CI status reported honestly (no CI configured).
- TDD: RED before GREEN for behavior changes; upstream semantics mirrored
  from the pinned upstream at `085c84e` (read-only reference).

## Tasks

| ID | Spec | WP | Issue | Route | Commit/PR |
|----|------|----|-------|-------|-----------|
| T1 | S2 | 501 | #34 | subagent writer + parent verify | `f46d1f9`+docs, PR 1 |
| T2 | S3 | 502 | #35 | subagent writer + parent verify | `7e0f614`, PR 2 |
| T3 | S4 | 503 | #36 | subagent writer + parent verify | `a41d7c2`+docs, PR 3 |
| T4 | S5 | 504 | #37 | writer tests + parent inline impl | `d9f07c1`+docs, PR 4 |
| T5 | S6 | 505 | #38 | parent inline (small WP) | `e2b91d3`+docs, PR 5 |
| T6 | S7 | 506 | #39 | parent inline (small WP) | `5cb2203`+docs, PR 6 |

## Log

- L1 (user, verbatim): "Starte mit M5"
- L2 (user, verbatim): "Arbeite autonom die Roadmap ab. Jeder Milestone wird ein Stack, wenn möglich mit main als base, und jedes WP wird ein PR im Stack. Jedes WP soll in möglichst kleine Arbeitspakete aufgeteilt werden welche dann, abgearbeitet, als commit im jeweiligen Branch/PR commited und gepushed werden."
- L3: M4 delivered (PR #29 merged as 1fca5ce); dependency repairs #31/#32/#33
  merged; main green (3feff03), suite 56 in-crate + 19 e2e + 42 smoke.
- L4: Execution order by dependency: 501 → 502 → 503 → 504 → 505 → 506.
- L5 (T6/WP-506): tool registry parity delivered inline (small WP). Golden
  schemas generated from the upstream dist at the pinned commit (4 files in
  tests/fixtures/tool-schemas/); built-in schemas() now byte-equal (JSON
  value equality, golden-diff test); read/write descriptions and the
  offset/limit types (number, matching upstream TypeBox) corrected. Merge
  follows upstream _refreshToolRegistry: extension tools shadow built-ins by
  name (shadowed entry keeps its position, new ext tools append) — pure
  function tools::merge_with_extensions (unit test) used by the model tool
  set, selectedTools, and run_tool dispatch (extension first). Shadowing
  e2e: fixture shadow-read.ts registers `read`, execution reaches the
  extension (result_summary "shadow-read:x"). Suite: 141 in-crate + 20 e2e
  + 42 smoke green. M5 complete: 6 WPs, 6 PRs in stack #42.
- L6 (T5/WP-505): headless permission defaults delivered inline (small WP,
  no writer trigger). Core now advertises `ui: []` (headless); the sidecar
  wires the dialog UI context only for clients that advertise UI methods,
  otherwise keeps the upstream noOp UI context (hasUI=false). Unmodified
  permission-gate.ts now blocks `sudo true` with the upstream no-UI default
  `Dangerous command blocked (no UI for confirmation)` (e2e matrix re-run
  green); default-allow for safe commands unchanged. Decision: `ca.initTheme()`
  in the headless path because the noOp context's theme getter resolves the
  shared theme — upstream only inits it in TUI startup, but headless-safe
  extensions (plan-mode, M1 matrix) call theme helpers in commands and the
  M1 headless-safe contract (user's core goal) keeps them working. Smoke
  (UI-client emulation) unchanged and green — dialog path stays for M6 TUI.
  Suite: 139 in-crate + 19 e2e + 42 smoke green.
- L6 (T4/WP-504): bash hardening delivered. Writer produced the 14 contract
  tests (RED) then stalled twice on the implementation (harness stall, 4 min
  "generating tool arguments" — incident); parent finished the implementation
  inline against the pinned test contract + upstream code. 10MB tail
  truncation with rolling 50KB+1 window (bounded memory), pi-bash-*.log full
  output, timeout kill with upstream validation strings, 128+signal exit
  codes, merged stdout/stderr accumulator (biased stdout-first pump mirrors
  libuv handle order). Three test expectations corrected against upstream
  evidence: exit status keeps the trailing-newline + appendStatus doubling
  (session fixture shows three newlines), the 10MB single-line command
  needed 20971520 input bytes (yes x pairs), interleaving order made
  deterministic with sleeps (race otherwise, upstream included). Suite:
  139 in-crate + 19 e2e + 42 smoke green.
- L6 (T3/WP-503): subagent delivered edit-tool parity (43 table/unit tests,
  RED→GREEN, suite 125 in-crate + 19 e2e green). New src/edit_diff.rs (LCS-DP
  diff port, documented shortcut: Myers beyond ~5k lines); exact upstream
  error/result strings pinned (unique/ambiguous/overlap/empty/no-op,
  CRLF/BOM/lone-CR preservation). Slice boundary: the algorithm port + its
  43 tests form one cohesive work unit (single behavior, WP-503 Size M) —
  recorded per house delivery rule. First writer attempt stalled at 4 min;
  relaunched with declared partial state.
- L6 (T2/WP-502): subagent delivered write-tool parity (7 unit tests, RED→GREEN,
  suite 81 in-crate + 19 e2e green). Success string `Successfully wrote to
  <raw input path>` (upstream quirk pinned); EACCES/EISDIR/mkdir Node-shaped
  errors; file-mutation queue + abort signals skipped (sequential execution,
  no abort plumbing in v0). Simplifications pinned in the PR.
- L6 (T1/WP-501): subagent delivered read-tool parity (18 contract tests,
  RED→GREEN, full suite 74 in-crate + 19 e2e green). Slice boundary: the
  read implementation + its 18 tests form one cohesive work unit (584
  authored lines in src/tools.rs, single file) — recorded per house
  delivery rule instead of splitting hunks of one file. Simplifications
  pinned in code comments: whole-file buffer read (upstream's own tests
  define the reference), M4 magic-byte image set, no resize pipeline.
  WP-505's TUI confirm path belongs to WP-604 (M6); M5 delivers the
  default-allow + extension-gate contract.

## Rebase onto post-M6 main (2026-11-12)

Stack #42 rebased from the pre-M6 main (3feff03) onto the new main
(11ba5c2, M6 merged via PR #67). Chain rebuilt in order (wp-501 onto
main, each next onto its rebased predecessor); each branch kept
exactly its own commits (3/2/1/2/1/2). Two M5xM6 integration
collisions surfaced only once both milestones stacked (M6 was built on
pre-M5 main):
1. sidecar/bridge.mjs: M6/WP-607 added the unconditional
   createDialogManager(+sendRequest) while M5/WP-505 kept its
   conditional UI-wiring block — the cherry-pick merged both, and the
   WP-505 block re-created the dialog WITHOUT sendRequest (smoke theme
   probe + PTY dialog test failed). Fix: one wiring — the WP-505
   conditional block now carries sendRequest; headless clients keep
   the noOp UI context, TUI clients get the dialog manager.
2. src/cli.rs init capabilities: WP-505 advertises ui:[] (correct for
   headless) which breaks M6/WP-604 TUI dialog forwarding. Fix:
   ui methods are advertised only in TUI mode (ui_methods =
   tui_mode ? [select,confirm,input,editor,notify] : []).
Verified at the stack top (feat/wp-506-tool-registry = M5+M6
superset): 226 in-crate + 23 e2e + 3 PTY + 43 sidecar smoke green.
Branches re-pushed (user-approved force, via normal push to temp refs
+ API ref update); all six PRs report mergeable_state clean.
