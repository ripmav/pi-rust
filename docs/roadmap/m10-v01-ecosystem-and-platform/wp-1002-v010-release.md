# WP-1002 — v0.1.0 release

**Milestone:** M10 · **Size:** S · **Status:** planned · **Depends on:** 1001

## Goal
Cut the first real release (`v0.1.0`) on the refreshed pin — the staging
pipeline (M9/WP-903) is proven; this converts it into a stable tag.

## Scope
- Tag `v0.1.0` on main after 1001; release workflow produces the four
  targets + `install.sh` (linux-gnu, linux-musl, darwin ×2, aarch64 via
  QEMU-smoke in CI).
- Release notes (from commits since `v0.0.0-staging.8`: M1–M9 headline
  features, install steps, known limitations from the README troubleshooting
  section).
- End-to-end verification: `install.sh` from the published release on a
  clean container (extend the existing fresh-clone CI job or one local
  docker run), plus `pi-rust --version`/`--bench`/one fake-provider turn.
- README: "Install from release" becomes the primary path.

## Acceptance
- Release `v0.1.0` published: 4 tarballs + install.sh, all smoke-passing in
  CI.
- Clean-container install→run verified from the real release artifact.
- Release notes published with the release.

## Out of scope
- `install.sh` signing (WP-906 LOW — needs a signing identity; post-M10).
- Windows target (WP-1008 adds it; v0.1.0 stays the four proven targets).

## Evidence (2026-11-14)

- Release `v0.1.0` published (run 43606008087, green): 4 tarballs +
  install.sh; per-target smoke in CI (aarch64 under QEMU).
- Curated notes via RELEASE_NOTES.md (workflow uses it when present at the
  tagged commit; commit-list fallback kept).
- `--version` flag added on this branch (WP-1002 delta): prints
  `pi-rust 0.1.0`; USAGE + docs/cli-reference.md fence updated in
  lockstep (cli_help diff-test green). The published v0.1.0 tag points
  at the release-prep commit (GitHub refuses tag deletion while a
  release exists and force-push is guarded), so `--version` ships with
  v0.2.0 (WP-1010); the published v0.1.0 notes make no --version claim.
- Install→run from the **published artifact** (no container runtime on the
  dev box — the CI fresh-clone job + release build jobs are the clean-OS
  proof): isolated scratch HOME/PI_RUST_INSTALL_ROOT/PI_RUST_BIN_DIR,
  install.sh from the release download URL, then `--version` + one-shot
  fake turn (session file written, clean exit) — green.
