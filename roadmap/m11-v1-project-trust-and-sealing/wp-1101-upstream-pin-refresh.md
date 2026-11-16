# WP-1101 — Upstream pin refresh (17d6ca7 → 64a15f4)

**Milestone:** M11 · **Size:** M · **Status:** planned · **Depends on:** M10 (done)

## Goal
Bump the upstream pin from `17d6ca759a070c377147f6d76b72dab9a3ba42a3` to
`64a15f4bee63d761d5398bf4126ea63abef0dc98` (origin/main, 2026-11-16, 73
commits ahead) and prove the compatibility layer still holds: the
unmodified-extension matrix, the sidecar smoke, and the full suite green on
the new pin.

## Scope
- Fetch upstream, bump `upstream/` to the new SHA; update every pin
  reference (README ×5, ci.yml + release.yml `UPSTREAM_SHA`,
  docs/bridge-protocol.md, code comments in tests/rpc.rs,
  tests/extension_bridge.rs, src/tools.rs, src/skills.rs + any others the
  M10 sweep found).
- `npm install + build` in `upstream/`; inspect the diff of the pinned
  packages (ai, coding-agent, tui, mcp) for contract changes the sidecar
  consumes.
- Migrate the sidecar/core wherever the new pin changed a consumed contract
  (protocol doc updated in lockstep if bridge payloads change).
- Re-record `benches/baseline.json` (median of 3, same method as the M9
  median gate) on the new pin.
- Re-run the unmodified-extension matrix and the full test suite.

## Upstream delta (explored 2026-11-16)
- **Consumed packages: minimal.** `coding-agent` src/dist: **unchanged**.
  `tui`: **unchanged**. `ai`: 2 files / +9-3 — Mistral conversations
  strict-mode fix (`compat.supportsStrictMode`, default false: "Mistral
  Large 4 can go silent for minutes before a strict tool call").
  `mcp`: unchanged. Repo-wide: 178 files / +19,001-1,058, dominated by
  internal packages (`p2`, `p2-mac`, `p3`, `chord`, `durable`) — all
  documented v1 non-goals for pi-rust (README "Known v0 non-goals (v1)").
- **Parity assessment:**
  - Mistral strict-mode: **not a gap** — pi-rust sends tool definitions
    without a `strict` field (openai.rs `tools_from`), matching the
    post-fix upstream default (opt-in, default false). Document only.
  - `undici 8.12.4` security bump (coding-agent dep): rides along in the
    sidecar tree via the pin; no Rust-side action.
  - `durable` image processing, `chord` fixes, p2/p3 features: out of
    scope (internal/experimental).
- Expected code migration: **zero** (matrix + suite are the proof).

## Acceptance
- `upstream/` at the new SHA; all pin references updated and consistent.
- Export matrix re-verified (157 coding-agent exports at planning; any
  drift documented).
- Matrix + smoke + full suite green on the new pin; fmt + clippy clean.
- Bench baseline re-recorded (3-sample median); CI bench job green.

## Out of scope
- Adopting new upstream *features* (p2/p3/chord/durable) — v1 non-goals.
- Any M11 feature WPs (1102–1104).
