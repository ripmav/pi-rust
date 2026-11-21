# WP-1217 — Scaffolding + authoring docs

**Milestone:** M12 · **Story:** E1S5 · **Size:** M · **Status:** open · **Depends on:** WP-1205, WP-1210

## Goal
Making a new user extension take minutes, not an afternoon
([ADR 0003](../../adr/0003-workspace-with-pi-ext-crate.md)).

## Scope
- `pi-rust ext new <name>` (or a cargo template — design in the WP):
  working skeleton (tool + command + event handler) + `pi-ext` dependency +
  `pi-ext.toml` manifest + test-harness wiring.
- Authoring docs in `docs/`: discovery paths (agent dir, `.pi/extensions`,
  `--extension-dir`, `pi-ext.toml`), the trust gate (project scope), the UI
  surface + no-UI defaults, versioning (`pi-ext` 0.x policy,
  `protocolVersion`), running an extension standalone (NDJSON harness).

## Acceptance
- The scaffolded extension compiles and passes the extension test harness
  out of the box (tested by the e2e: scaffold → build → harness green).
- Docs cover discovery / trust / UI / versioning / standalone running.
