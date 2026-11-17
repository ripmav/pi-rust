# WP-1102 — Project trust model

**Milestone:** M11 · **Size:** L · **Status:** planned · **Depends on:** 1101

## Goal
A minimal, explicit trust gate for project-local configuration in
`<cwd>/.pi/`. Today, project-level files are a documented non-goal because
they would execute or apply untrusted content (WP-307: `.pi/settings.json`
merge; WP-1009 T9a: `.pi/mcp.json`). This WP builds the gate, not the
features; WP-1103 turns them on under it.

## Design decisions to settle in this WP
- Trust representation: per-project marker (e.g. `<cwd>/.pi/trusted` file
  or an agent-dir allowlist entry keyed by project path hash) — decide and
  document.
- How a project gets trusted: explicit user action only (CLI flag
  `--trust-project`? TUI confirmation? agent-dir config entry?). No
  implicit trust, no network-sourced trust.
- What the gate covers: `.pi/settings.json`, `.pi/mcp.json`, and project
  skills (`<cwd>/.pi/skills/`) — the three project surfaces that read or
  execute content from the cwd.
- Security properties: an untrusted project changes **zero** behavior vs.
  today; trust is cwd-scoped (never inherited by subdirectories or
  siblings); the trust state survives across sessions (persistent) and is
  visible/inspectable.

## Scope
- Trust state storage + lookup (agent-dir side, path-canonicalized).
- CLI/TUI surface for granting + listing + revoking trust.
- Gate hooks where project files are read (settings load, mcp config load,
  skills scan).
- Tests: untrusted cwd changes nothing; trusted cwd applies project config;
  trust is per-path (not prefix/pattern); revocation reverts.

## Acceptance
- Untrusted `<cwd>/.pi/` content has zero behavioral effect (proven by
  test: identical session init with/without project files).
- Granting trust is an explicit, persistent, user-visible action.
- Docs: README + cli-reference updated; WP-307 and WP-1009 T9a notes
  superseded.

## Out of scope
- Enabling the features themselves (WP-1103).
- Upstream project-trust features (none exist at the pin — pi-rust design).
