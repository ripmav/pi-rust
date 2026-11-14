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
