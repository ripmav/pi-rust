# WP-905 — Performance baseline

**Milestone:** M9 · **Size:** S · **Status:** open · **Depends on:** 901

## Goal
Measurable baselines: cold start to `ready` (target < 500ms without model
I/O), memory at idle, throughput of the NDJSON bridge (events/s). Recorded
in CI (comparison against baseline commit).

## Acceptance
- Benchmark job runs on PRs; regressions > 20% fail the check; baseline
  committed.

## Out of scope
- Profiling beyond finding the first real bottleneck.
