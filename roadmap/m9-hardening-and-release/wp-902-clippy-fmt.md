# WP-902 — clippy + rustfmt baseline

**Milestone:** M9 · **Size:** S · **Status:** open · **Depends on:** —

## Goal
`cargo clippy -- -D warnings` and `cargo fmt --check` pass; fix or allow
with justification; both join CI (WP-901).

## Acceptance
- Clean run on HEAD; CI enforces.

## Out of scope
- New lint categories beyond clippy defaults.
