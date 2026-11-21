# WP-907 — Documentation

**Milestone:** M9 · **Size:** M · **Status:** delivered (2026-11-14, PR #116) · **Depends on:** M6

## Goal
README becomes the real docs: install (release + from source), CLI reference
(auto-generated from arg parser), provider table (all WPs in M4), extension
compatibility guide (with the contract table from 00-vision), troubleshooting.

## Acceptance
- Fresh-clone setup works by following the README alone (tested on a clean
  container); CLI reference matches actual flags (generated or diff-tested).

## Out of scope
- Docs site hosting.

## Delivery memo (2026-11-14)

- README: install (release + from source), CLI reference diff, provider
  table (existing, kept), extension compatibility guide (00-vision table
  with delivered statuses), troubleshooting, roadmap status.
- `--help`/`-h` added; `docs/cli-reference.md` = exact --help output,
  pinned by `tests/cli_help.rs` (drift fails CI).
- CI job `fresh-clone`: clean container (node:26-bookworm) follows the
  README recipe verbatim + smoke subset — the acceptance's "clean
  container" proof.
- Local: full suite green; fresh-clone recipe run in a temp clone
  (pre-PR confidence; the container job is binding).

## Native review of the M9 stack head (2026-11-14)

- Range: 36a8e28..907 head (12 paths, 612 lines), lineage
  `review-265b589651f56ede`, tier high, lenses readability +
  reliability, **approved** (authority burned, store revision
  `12153d30`).
- Advisory findings (non-blocking; separate later work):
  - R2-001 (SUGGESTION) `.github/workflows/ci.yml:196` (readability)
  - R2-002 (WARNING) `src/config.rs:330-332` (readability)
  - R2-003 (SUGGESTION) `src/tools.rs:141-150` (readability)
  - R2-004 (SUGGESTION) `.github/workflows/ci.yml:160` (readability)
  - R3-UPSTREAM-SHA (WARNING) `.github/workflows/ci.yml:210`
    (reliability)
