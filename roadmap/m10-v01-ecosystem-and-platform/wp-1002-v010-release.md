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
