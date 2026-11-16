# WP-1103 — Project `.pi/` config under the trust gate

**Milestone:** M11 · **Size:** M · **Status:** planned · **Depends on:** 1102

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
