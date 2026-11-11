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
| T3 | S4 | 503 | — | subagent writer + parent verify | — |
| T4 | S5 | 504 | — | subagent writer + parent verify | — |
| T5 | S6 | 505 | — | subagent writer + parent verify | — |
| T6 | S7 | 506 | — | subagent writer + parent verify | — |

## Log

- L1 (user, verbatim): "Starte mit M5"
- L2 (user, verbatim): "Arbeite autonom die Roadmap ab. Jeder Milestone wird ein Stack, wenn möglich mit main als base, und jedes WP wird ein PR im Stack. Jedes WP soll in möglichst kleine Arbeitspakete aufgeteilt werden welche dann, abgearbeitet, als commit im jeweiligen Branch/PR commited und gepushed werden."
- L3: M4 delivered (PR #29 merged as 1fca5ce); dependency repairs #31/#32/#33
  merged; main green (3feff03), suite 56 in-crate + 19 e2e + 42 smoke.
- L4: Execution order by dependency: 501 → 502 → 503 → 504 → 505 → 506.
- L5 (T2/WP-502): subagent delivered write-tool parity (7 unit tests, RED→GREEN,
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
