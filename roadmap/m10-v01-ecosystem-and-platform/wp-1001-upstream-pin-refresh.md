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

## Evidence (2026-11-14)

Pin target: `17d6ca759a070c377147f6d76b72dab9a3ba42a3` (upstream origin/main at
planning+1: p2 theme-fade fix over `8636e21`).

Contract inspection (pin → 17d6ca7, consumed packages: 61 files, 57 M / 4 A):
- **dist/index.js export surface: unchanged** (157 exports, 0 added / 0
  removed — rebuilt and diffed).
- `packages/ai/src/types.ts` (AssistantMessageEvent union): **unchanged** —
  the sidecar's event mapping contract holds.
- Direct dist imports: `core/system-prompt.js` (`buildSystemPrompt`,
  `normalizeBuildSystemPromptOptions`) and `core/mcp-servers.js`
  (`validateMcpServerConfig`): **files unchanged**.
- `@earendil-works/pi-mcp`: additive — new `mcpServerSummary` export; the
  mcp extension's local `serverSummary` was refactored onto it (identical
  behavior, 5+/8-).
- `@earendil-works/pi-ai`: additive `cache-warming` export; provider files
  changed internally (retry/params) but no event-shape changes.
- Unchanged: extension types, loader, skills, slash-commands, keybindings,
  frontmatter, theme tokens, runner.
- Expected code migration: **zero** (matrix + suite are the proof).

Pin references bumped in: README.md, .github/workflows/{ci,release}.yml
(incl. rust-cache key), tests/{rpc,extension_bridge}.rs, docs/bridge-protocol.md,
src/{tools,skills,session,provider/{mod,google,bedrock}}.rs (comments),
roadmap/00-vision.md. Historical WP memos keep the old SHA (they document
what held at that pin).

Status: pin bumped, full suite running on the new pin (evidence pending).

## Out of scope
- Adopting new upstream *features* (p2 themes, per-location MCP, appearance
  settings) — evaluated in WP-1009, implemented only if selected there.
- New extension capabilities (M10 WPs 1003–1007).

### Completion (2026-11-14)
- Full suite on the new pin: 271 in-crate, 1 cli-help, 38 e2e
  (unmodified-extension matrix), 4 rpc, 3 PTY, 3 skills — all green.
- Sidecar smoke: 68 passed, 0 failed.
- Bench baseline re-recorded (3-sample median on the new pin):
  cold_start 1070.6 ms, idle_rss 5.2 MB, events/s 6299.0 (samples:
  5589/7582.8/6299, 1038.4/1070.6/1088.9, 5.2/5.2/5.2).
- Milestone entry decision: **bump** (compatibility layer intact — no
  contract migration needed).
