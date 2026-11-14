# WP-905 — Performance baseline

**Milestone:** M9 · **Size:** S · **Status:** delivered (2026-11-14, PR #114) · **Depends on:** 901

## Goal
Measurable baselines: cold start to `ready` (target < 500ms without model
I/O), memory at idle, throughput of the NDJSON bridge (events/s). Recorded
in CI (comparison against baseline commit).

## Acceptance
- Benchmark job runs on PRs; regressions > 20% fail the check; baseline
  committed.

## Out of scope
- Profiling beyond finding the first real bottleneck.

## Bench band memo (2026-11-14)

- The flat 20% band flaked: `idle_rss_mb` measured 6.5 vs baseline 5.0
  on one runner (PR #115 run) while the identical code passed on
  another (PR #116 run) — shared-runner RSS jitters ±1.5 MB.
- Fix: per-metric band — timing/throughput stay at 20%, `idle_rss_mb`
  gets 50% (still catches real allocation regressions, verified:
  9.0 MB vs 5.0 baseline still fails, throughput −30% still fails).
- Baseline refresh policy: re-record on toolchain bumps / runner
  generation changes, not on every flake.
