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

## Acceptance
- Every advisory line has a resolution (fix + test, or re-documented
  non-goal with rationale) — none silently dropped.
- Suite + matrix + fresh-clone green at the M10 head.
- M10 marked delivered in `roadmap/01-milestones.md`; release notes for
  `v0.2.0` published (if released).

## Out of scope
- New features (M11 planning is a separate decision).
