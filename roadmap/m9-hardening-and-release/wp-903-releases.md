# WP-903 — Release workflow

**Milestone:** M9 · **Size:** S · **Status:** delivered (2026-11-14, PR #112; staging.8 published, all 4 targets + install.sh smoke-tested incl. aarch64 under qemu) · **Depends on:** 901

## Goal
Tag → GitHub release with signed-ish artifacts (x86_64/aarch64 linux+macos),
install script, changelog from merged PRs.

## Acceptance
- Test release from a staging tag installs and runs (smoke scenario) on
  both architectures; artifacts < size budget (strip debug).

## Out of scope
- Homebrew/aur (community later).
