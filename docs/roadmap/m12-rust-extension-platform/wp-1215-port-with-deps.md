# WP-1215 — Port `with-deps`

**Milestone:** M12 · **Story:** E1S4 · **Size:** S · **Status:** open · **Depends on:** WP-1210

## Goal
The dependency story: a core extension with its **own external dependency**
(npm `ms` → a Cargo crate), proving the `core-ext` crates are normal
standalone cargo projects ([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).

## Scope
- `core-ext/with-deps`: the `parse_duration` tool (same name, label,
  description, parameters) using a Cargo duration-parsing crate (e.g.
  `humantime`); invalid input → same error behavior.

## Acceptance
- Same e2e scenario as the unmodified original: `1h` →
  `1h = 3600000 milliseconds`; invalid duration → error.
- The crate's own `Cargo.toml` declares the external dependency (not the
  workspace root).
