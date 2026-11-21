# 2. Mirror the full upstream extension API 1:1 in v1

Date: 2026-11-21

## Status

Accepted

## Context

Epic E1 defines the Rust extension surface (E1S2) and ports existing TS
extensions (E1S4). This ADR settles D2 (which API surface the interface
exposes in v1).

The upstream extension API (`pi` object + `ctx`,
`upstream/packages/coding-agent/src/core/extensions/types.ts`) is the standard
existing extensions are written against; the porting goal ("old ones ported to
Rust") is credible only if that mapping is mechanical. The bridge v6 catalog
already carries the **full** surface — including the TUI-bound parts (dialogs,
renderers/custom pane via the `ui` capabilities, shortcuts/keybindings) — so
mirroring it adds no new wire work, only Rust-side types.

The alternative (headless-safe slice first, TUI parts later) would leave ports
incomplete and force a second API shape onto the framework crate later — a
breaking rework of exactly the surface that is supposed to be stable.

## Decision

The `pi-ext` crate ([ADR 0003](./0003-workspace-with-pi-ext-crate.md)) exposes a **1:1 mirror of the upstream
`pi` + `ctx` API** in v1: event handlers (`on`, all catalog events), tool
registration (JSON-schema parameters, `exposure`, `defaultActive`), commands,
shortcuts/keybindings, flags, `sendUserMessage`/`appendEntry`, session name and
label, `exec`, settings access, model and thinking-level selection, provider
registration, MCP registration, and the UI surface (`select`/`confirm`/
`input`/`editor`/`notify`/`custom`).

Parity rule: no capability exists in the framework that the bridge v6 catalog
cannot express, and no v6-catalog capability is absent from the framework. A
parity check (catalog ↔ API diff) is a test in the crate.

## Consequences

- E1S4 ports are mechanical: each upstream API call maps 1:1, and the porting
  guide is a mapping table, not an adaptation story.
- E1S2 is bigger from day one (the full type surface), but there is no second
  API shape and no breaking rework when TUI parts are exercised.
- TUI-bound APIs are available in v1; they are exercised by the TUI e2e in
  E1S3 (UI attached) and by the TUI e2e of the ported extensions.
- The parity test keeps the mirror honest across upstream pin refreshes (the
  M11 pin cycle re-verifies it).
