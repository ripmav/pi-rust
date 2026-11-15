# WP-1010 — M10 close (advisory sweep + verification)

**Milestone:** M10 · **Size:** S · **Status:** planned · **Depends on:** 1002–1009

## Goal
Close M10: clear the known non-blocking follow-ups, verify the whole
milestone, and release `v0.2.0` if the suite warrants it.

## Scope
- Advisory sweep (each: fix or re-document with rationale):
  - Stack-head review advisories: R2-001, R2-004 (ci.yml), R2-002
    (config.rs TOCTOU — accepted v0 limitation unless trivially fixable),
    R2-003 (tools.rs), R3-UPSTREAM-SHA (single-source the upstream SHA —
    README, ci.yml, release notes read one canonical source).
  - WP-906 LOWs: install.sh signing decision (needs identity — likely
    re-document), BashCapture leak (may be done in 1009 — confirm).
- Cleanup: delete the tangled M9 stacked branches (905/906/907 — content
  fully in main); keep the renovate branches.
- Verification: full suite at the M10 head (in-crate, e2e, rpc, PTY,
  skills, cli_help, sidecar smoke), unmodified-extension matrix on the
  final pin, fresh-clone CI job.
- Docs: README (install, compatibility table, new capabilities),
  milestone doc statuses, `01-milestones.md` M10 delivered.
- Release: `v0.2.0` tag via the proven workflow (5 targets incl. Windows)
  with release notes, if the suite is green.

## Advisory resolutions (WP-1010 sweep, 2026-11-15)

| Advisory | Location (at review time) | Resolution |
| --- | --- | --- |
| R2-001 (SUGGESTION, readability) | ci.yml test-job upstream block | **fixed**: upstream setup extracted into `.github/actions/upstream-setup` (single copy, used by test matrix + bench; the two copies had diverged — the bench job missed the WP-1009 verify/repair) |
| R2-004 (SUGGESTION, readability) | ci.yml bench-job upstream block | **fixed**: same composite action (see R2-001) |
| R2-002 (WARNING, readability) | src/config.rs ensure_agent_dir | **fixed**: the `create_dir` result is now the authoritative "we created it" signal (0700 applied exactly when pi-rust created the leaf); the `exists()` pre-check is gone; existing tests (fresh→0700, existing→keeps mode) still pin the behavior |
| R2-003 (SUGGESTION, readability) | src/tools.rs BashCapture block | **re-documented**: the block the advisory flagged is now the `finished` flag + `Drop` impl with the WP-906/WP-1009 rationale comments; no further action |
| R3-UPSTREAM-SHA (WARNING, reliability) | ci.yml cache key | **fixed (machinery)**: cache keys are now derived from the workflow-level `UPSTREAM_SHA` env (full SHA in the key) instead of a hardcoded 8-char fragment that could drift; both workflows single-source their SHA via that env. README/RELEASE_NOTES remain human docs stating the current pin (kept consistent by process at pin bumps). |
| WP-906 LOW: install.sh unsigned | install.sh | **re-documented (v0 decision)**: no signing identity for a personal OSS release in v0; the install script is fetched from the GitHub release of the pinned repo and the README documents the checksum-verified tarball layout as the integrity story. Revisit with a signing identity if one exists. |
| WP-906 LOW: BashCapture leak on abort | src/tools.rs | **fixed in WP-1009** (`f709caa`): Drop removes the temp log; truncated captures keep it for `fullOutputPath`. |
| R3-001 (WP-906 lineage, TOCTOU/intermediate dirs in ensure_agent_dir) | src/config.rs | **accepted v0 limitation** (recorded at M9): the 0700 applies to the leaf pi-rust creates; intermediate dirs (e.g. `~/.pi`) and the create-window race are out of scope for a personal tool. The R2-002 refactor removed the extra `exists()` pre-check, narrowing the window. |

## Cleanup
- Tangled M9 stacked branches (905/906/907) deleted local + remote (content fully in main); renovate branches kept.

## Verification + release
- Full suite + matrix + fresh-clone at the M10 head (CI).
- M10 release via the proven 5-target release workflow, with release
  notes. (The v0.2.0 tag published once, was caught by the acceptance
  check — `--version` printed 0.1.0, Cargo.toml never bumped — and was
  retracted; GitHub tag immutability + semver make **v0.2.1** the M10
  release. See feature doc L14.)

## Acceptance
- Every advisory line has a resolution (fix + test, or re-documented
  non-goal with rationale) — none silently dropped.
- Suite + matrix + fresh-clone green at the M10 head.
- M10 marked delivered in `roadmap/01-milestones.md`; release notes for
  `v0.2.0` published (if released).

## Out of scope
- New features (M11 planning is a separate decision).
