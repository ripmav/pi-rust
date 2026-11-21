# 5. Port the six bridge-matrix examples as core extensions

Date: 2026-11-21

## Status

Accepted

## Context

E1S4 needs a porting set (D5). The six bridge-matrix examples are the only
upstream examples with green e2e evidence against the **unmodified** original
([tests/extension_bridge.rs](https://github.com/ripmav/pi-rust/blob/main/tests/extension_bridge.rs)
+ [tests/tui.rs](https://github.com/ripmav/pi-rust/blob/main/tests/tui.rs)) —
which makes the acceptance mechanically checkable ("the port passes the same
scenario as the original") and keeps a permanent A/B baseline (the original TS
keeps running through the sidecar in a mixed session, E1S3).

The six cover the API in layers:

- `hello` — tool registration + JSON-schema parameters
- `permission-gate` — blocking decision (`tool_call` block/reason) + UI dialog + the headless-safe default
- `claude-rules` — file I/O in an extension + state across events + system-prompt injection
- `prompt-customizer` — `systemPromptOptions.sections` mutation
- `with-deps` — an extension with its **own external dependency** (npm `ms` → a Cargo crate)
- `plan-mode` (598 lines) — the deep API: flags, commands, shortcuts, tool-set management, session persistence/resume, custom UI (status/widget/editor/select)

The remaining ~60 upstream examples are TUI renderer apps, real custom
providers, and network tools — applications that *use* the API, not references
*for* it. `subagent/` stays out (spawns the `pi` binary, not headless-safe —
M3 decision).

The user additionally decided that the six ports are not merely reference
examples but **core extensions** — following the existing built-in extension
precedent (the `llama` extension loaded from the shipped DIST, WP-1005; the
built-in MCP extension, WP-702). This ADR settles D5.

## Decision

- **Porting set (v1):** exactly the six — `hello`, `permission-gate`,
  `claude-rules`, `prompt-customizer`, `with-deps`, `plan-mode`.
- Each port must pass the **same e2e scenarios** as the unmodified original
  (same fixtures; the scenarios are already green for the original).
- The six ports are **core extensions**: bundled with the pi-rust release
  (installed alongside the binary, per the `install.sh` / `find_sidecar`
  pattern), **loaded by default in every session**, and in **core scope** —
  trusted by definition (the trust gate of [ADR 0004](./0004-rust-extensions-share-discovery-and-trust.md)
  covers the user/placement locations; core extensions come from the install
  location).
- `with-deps` carries its own external dependency (npm `ms` → a Cargo crate)
  and proves the dependency story end to end.
- The on-disk layout of core extensions, their build/install steps, and any
  user opt-out: defined in E1S1's architecture doc.

## Consequences

- E1S4's acceptance = six ports × same scenarios + the bundled
  default-load behavior; the porting guide gets one section per extension,
  including the hard parts (plan-mode's resume scan, with-deps' dependency
  mapping).
- Discovery gains a source besides [ADR 0004](./0004-rust-extensions-share-discovery-and-trust.md)'s
  three user locations: the **core install location** (always trusted, loaded
  by default). E1S3's runtime routes three sources: core install (Rust,
  default), sidecar TS user locations, Rust user locations.
- Release artifacts grow by the core-extension binaries (per target; shape
  defined by the architecture doc); `install.sh` and CI packaging must cover
  them.
