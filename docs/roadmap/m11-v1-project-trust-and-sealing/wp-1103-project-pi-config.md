# WP-1103 — Project `.pi/` config under the trust gate

**Milestone:** M11 · **Size:** M · **Status:** delivered — merged into main as `cb4b47c` (PR #148, 2026-11-18) · **Depends on:** 1102

## Goal
Turn on the two documented project-config non-goals, gated by the WP-1102
trust model: `.pi/settings.json` merge (WP-307) and project
`<cwd>/.pi/mcp.json` (WP-1009 T9a).

## Scope
- **Settings merge:** project `.pi/settings.json` merged over the
  agent-dir settings with the documented precedence (agent-dir base,
  project overrides), same merge semantics as the upstream settings layer;
  only when the project is trusted.
- **Project MCP:** `<cwd>/.pi/mcp.json` servers added to the host-fed MCP
  config, only when trusted; same validation as agent-dir mcp.json
  (`validateMcpServerConfig`).
- Tests for both surfaces: trusted/untrusted, precedence, invalid project
  config rejected without breaking the session.
- Docs: README compatibility matrix rows move from "documented non-goal"
  to "delivered (trusted projects)"; cli-reference.

## Acceptance
- Trusted project: settings + MCP from `.pi/` applied (proven by E2E).
- Untrusted project: zero effect (proven by test).
- Suite green; fmt + clippy clean.

## Out of scope
- Project skills trust handling beyond the existing scan (already cwd-
  scoped; verify only).
- Per-key or per-server granular trust.

## Implementation record (2026-11-18)
- Core-side merge (no protocol change, sidecar byte-identical):
  - `merge_project_settings` (src/config.rs): `<cwd>/.pi/settings.json`
    over the agent-dir settings, per-field project-wins, trusted only;
    invalid file → Err naming the path; cli.rs logs the rejection and
    continues with the global settings (upstream: settings load errors
    are collected, never fatal).
  - `merge_project_mcp` (src/config.rs): upstream `loadMcpConfig`
    semantics — full entry replaces the same-named global server (drops
    its credentials); override entries (no `command`/`url`/`type`) are
    limited to `enabled`/`exposure`/`toolExposure` and merge over the
    global entry (credentials kept); override without a global base or
    with extra keys → rejected; `auth` on a project `url` server →
    rejected (global-only); `-`/`_` namespace clash → rejected; top-level
    `autoEnableCodemode` project boolean overrides the global; malformed
    project file → agent config returned unchanged + warning message.
  - cli.rs: both merges run after trust resolution, before
    `resolve_provider_flags` / the sidecar init; the init payload's
    `mcpConfig` now carries the merged config (doc updated).
- Design note: the upstream dist exports `loadMcpConfig`, which re-reads
  the files itself; reusing it would have needed an `agentDir` in the
  init payload (protocol change) plus duplicated fail-fast validation.
  Porting the merge to the core keeps the protocol at v6 and the
  sidecar untouched.
- Tests: 17 new unit (5 settings + 12 mcp, incl. untrusted = never
  read, credential preservation on override, all rejection paths) + 3
  new e2e (trusted project MCP round-trip via `mcp__fixture__echo`;
  untrusted → `Unknown extension tool`; invalid project settings →
  exit 0 + `project settings rejected` on stderr).
- Verification: fmt clean, clippy 0 warnings, full suite 394/0,
  sidecar smoke 74/74.
- Docs: README project-trust section (merge semantics) + compatibility
  matrix row, bridge-protocol `mcpConfig` field + server-sources line.
- The fake driver discards the resolved model id (`fake_mode`), so the
  per-field settings merge has no fake-mode behavior observable; it is
  unit-tested, and the e2e proves the real startup path reads the
  project file (non-fatal reject) and the MCP surface applies.
- Dead fields noted (pre-existing, untouched): `sessionDir` and
  `defaultTools` settings are parsed but never consumed.

## Delivery record (2026-11-18)
- PR #148 merged into main as `cb4b47c`; CI 7/7 green on the first run
  (44612758644, incl. bench baseline compare + fresh-clone) — the
  WP-1102 platform lessons (canonicalized expectations, parsed-JSON key
  checks, serde_json fixtures) meant no platform round.
