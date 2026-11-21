# WP-1212 — Port `permission-gate`

**Milestone:** M12 · **Story:** E1S4 · **Size:** S · **Status:** open · **Depends on:** WP-1210

## Goal
The blocking-decision + UI-dialog reference port
([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).

## Scope
- `core-ext/permission-gate`: same dangerous patterns (`rm -rf`, `sudo`,
  `chmod`/`chown` 777), same block reasons, same headless default (block
  with "no UI for confirmation" reason), same `ui.select` prompt when a UI
  is attached.

## Acceptance
- Same e2e scenarios as the unmodified original: dangerous bash blocked
  with the right reason (headless) and confirmed via dialog (TUI); the tool
  is not executed when blocked.
