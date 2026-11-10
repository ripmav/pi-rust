# WP-903 — Release workflow

**Milestone:** M9 · **Size:** S · **Status:** open · **Depends on:** 901

## Goal
Tag → GitHub release with signed-ish artifacts (x86_64/aarch64 linux+macos),
install script, changelog from merged PRs.

## Acceptance
- Test release from a staging tag installs and runs (smoke scenario) on
  both architectures; artifacts < size budget (strip debug).

## Out of scope
- Homebrew/aur (community later).
