# WP-1001 — Upstream pin refresh (085c84e → origin/main)

**Milestone:** M10 · **Size:** M–L · **Status:** planned · **Depends on:** M9 (done)

## Goal
Bump the upstream pin from `085c84e98e7f4c950f2be606466fff4f75d8fe7c` to the
latest `origin/main` (M10 planning: `8636e21`, 50+ commits behind) and prove
the compatibility layer still holds: the unmodified-extension matrix, the
sidecar smoke, and the full suite re-run green on the new pin.

## Scope
- Fetch upstream, bump `upstream/` to the new SHA; update every pin reference
  (README "From source", ci.yml fresh-clone job, release notes, feature docs).
- `npm install + build` in `upstream/`; inspect the diff of the pinned
  packages (ai, coding-agent, tui, mcp) for contract changes the sidecar
  consumes: extension types, runner API, dist exports, keybindings,
  frontmatter, theme tokens, json-event shapes.
- Migrate the sidecar/core wherever the new pin changed a consumed contract
  (protocol doc updated in lockstep if bridge payloads change).
- Re-record `benches/baseline.json` (median of 3, same method as the M9
  median gate) on the new pin.
- Re-run the unmodified-extension matrix (all M5–M8 fixtures + bundled
  extensions) and the full test suite.

## Acceptance
- `upstream/` at the new SHA; all pin references updated and consistent.
- Matrix + smoke + full suite green on the new pin.
- Bench baseline re-recorded (3-sample median); CI bench job green.
- Milestone doc records the keep/bump decision (M10 entry criterion).

## Out of scope
- Adopting new upstream *features* (p2 themes, per-location MCP, appearance
  settings) — evaluated in WP-1009, implemented only if selected there.
- New extension capabilities (M10 WPs 1003–1007).
