# WP-901 — CI pipeline

**Milestone:** M9 · **Size:** S · **Status:** open · **Depends on:** —

## Goal
GitHub Actions: `cargo build` + `cargo test` + clippy + `node
sidecar/smoke.mjs` (Node setup, upstream build cached) on PR + main push,
macOS + Linux.

## Acceptance
- Red-on-failure proof (deliberate break test run), green on HEAD; cached
  upstream build keeps CI < ~10 min.

## Out of scope
- Release publishing (WP-903).
