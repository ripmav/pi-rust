# WP-1204 — `pi-ext`: API surface — rest + parity

**Milestone:** M12 · **Story:** E1S2 · **Size:** M · **Status:** open · **Depends on:** WP-1203

## Goal
Complete the 1:1 mirror of the upstream API
([ADR 0002](../../adr/0002-mirror-full-upstream-extension-api.md)) and
prove it with the parity test.

## Scope
- `registerCommand`, `registerShortcut` (keybindings), `registerFlag` /
  `getFlag`.
- Provider registration (`registerProvider` + stream surface) and MCP
  registration.
- UI surface: `select` / `confirm` / `input` / `editor` / `notify` /
  `custom`, incl. `hasUI` and the no-UI defaults (bridge §init
  capabilities).
- **Parity test:** every bridge-v6-catalog capability exists in the API and
  no API capability is missing from the catalog (one test, catalog ↔ API
  diff).
- `pi-ext` 0.x section in [stability-contract.md](../../stability-contract.md)
  (semver policy for the extension API).

## Acceptance
- Parity test green.
- `stability-contract.md` updated; `cargo test --doc` still green.

## Out of scope
- Transports (WP-1205/1206).
