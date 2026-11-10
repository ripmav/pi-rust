# WP-504 — `bash` hardening

**Milestone:** M5 · **Size:** M · **Status:** open · **Depends on:** —

## Goal
Built-in `bash` reaches upstream semantics: timeout (config-driven,
WP-301), output truncation (last N lines/bytes), exit-code propagation,
`PI_*` env for the child, cwd = project.

## Acceptance
- Timeout kill test (1s limit), truncation golden test, non-zero exit
  reported with stderr; long-output memory bounded (10MB input test).

## Out of scope
- Interactive shells.
