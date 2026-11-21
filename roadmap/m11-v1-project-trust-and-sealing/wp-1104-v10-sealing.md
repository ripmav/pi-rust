# WP-1104 — v1.0 sealing & release

**Milestone:** M11 · **Size:** M · **Status:** delivered — v1.0.0 released (2026-11-21) · **Depends on:** 1101–1103

## Goal
Seal pi-rust as v1.0: a stability contract for the surface users rely on, a
complete CHANGELOG, and the v1.0.0 release on all five targets.

## Scope
- **Stability contract (doc):** what v1 freezes — CLI surface (flags,
  subcommands, exit codes, `--version`), bridge protocol v6 (versioned,
  additive-only), session file format, `~/.pi` layout, auth.json format —
  and the deprecation policy for breaking changes (major version +
  migration note in release notes).
- **CHANGELOG.md:** full history v0.0.0 → v1.0.0 from the release tags and
  milestone docs (curated, user-facing).
- **v1.0.0 release:** Cargo.toml bump, RELEASE_NOTES for v1.0.0, tag +
  release run on the 5 targets + install.sh, isolated-install acceptance
  check (`--version` → `pi-rust 1.0.0`).
- README: "Roadmap status" updated (M1–M11 delivered), v1 non-goals
  section re-based on what v1 actually froze.

## Acceptance
- Stability contract reviewed and committed (docs/).
- CHANGELOG covers every release since v0.0.0.
- v1.0.0 published, acceptance check green, M11 marked delivered in
  `roadmap/01-milestones.md`.

## Out of scope
- New features (backlog: full ui.custom component model, ARM64 Windows,
  v1 credential kinds — next milestone).

## Delivery record (2026-11-21)
- Sealing docs by gentle-ai-worker (CHANGELOG, stability contract,
  README, RELEASE_NOTES); parent: version bump 0.2.1 → 1.0.0.
- PR #149 merged as `9c7681f` (CI 7/7 on PR + main runs
  44885683585 / 44886403530); tag `v1.0.0` → release run 44886436938
  green (5 targets + install.sh, published 2026-11-21).
- Acceptance: published `pi-rust-1.0.0-aarch64-unknown-linux-musl.tar.gz`
  (native architecture) in an isolated temp dir → `pi-rust --version`
  → `pi-rust 1.0.0`. S5 met; M11 delivered.
