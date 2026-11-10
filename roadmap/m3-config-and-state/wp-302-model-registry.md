# WP-302 — Model registry (bundled catalog)

**Milestone:** M3 · **Size:** M · **Status:** open · **Depends on:** —

## Goal
Rust core ships/loads the upstream model catalog (all providers, from
`packages/ai` data) and can resolve `provider/model` ids to metadata
(context size, capabilities, cost, thinking support).

## Scope
- Generate a static catalog (build-time codegen from upstream `ai` data, or
  a vendored JSON) — no runtime npm dependency.
- `ModelRegistry::resolve(id)`, provider filtering, capability queries
  (`supports_tools`, `supports_thinking`, `max_tokens` default).

## Acceptance
- Catalog count matches upstream data at build time (test); resolution +
  capability tests for ≥3 providers; unknown id → typed error.

## Out of scope
- Custom model definitions from extensions (WP-701).
