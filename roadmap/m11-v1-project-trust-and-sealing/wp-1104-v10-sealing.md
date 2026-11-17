# WP-1104 — v1.0 sealing & release

**Milestone:** M11 · **Size:** M · **Status:** planned · **Depends on:** 1101–1103

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
