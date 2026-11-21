# WP-1211 — Mixed session + bench re-check

**Milestone:** M12 · **Story:** E1S3 · **Size:** S · **Status:** open · **Depends on:** WP-1207, WP-1208, WP-1210

## Goal
Prove all three extension sources in one session, and measure the
Rust-peer bridge throughput (the D1 performance re-check).

## Scope
- Mixed e2e: a TS extension (sidecar) + a Rust user extension (subprocess)
  + a core extension (in-process) in one session; tools from all three
  sources callable; the decision combination order observable.
- The `--bench` loop re-pointed at a Rust extension peer; record the
  measured `bridge_events_per_sec` in
  [benches/baseline.json](https://github.com/ripmav/pi-rust/blob/main/benches/baseline.json)
  (3-sample median, same method as the existing gate).

## Acceptance
- Mixed-session e2e green (new test in `tests/`).
- Measured Rust-peer events/s recorded in the baseline; the CI bench job
  compares it (direction-aware band, like the existing metrics).

## Out of scope
- Performance optimization (measure only).
