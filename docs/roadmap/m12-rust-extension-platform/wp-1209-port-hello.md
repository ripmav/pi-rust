# WP-1209 — Port `hello` (first core extension)

**Milestone:** M12 · **Story:** E1S4 · **Size:** S · **Status:** open · **Depends on:** WP-1206

## Goal
The first port ([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)):
`core-ext/hello` as a cdylib — the smallest end-to-end proof of the core
chain (cdylib → `pi-ext` core target → dlopen).

## Scope
- `core-ext/hello` crate (cdylib, depends on `pi-ext`): same tool name
  `hello`, label, description, parameters (`name: string`) and output text
  as the upstream `hello.ts`.
- Wired on the `pi-ext` core target (WP-1206).

## Acceptance
- The same e2e scenario as the unmodified original
  (`extension_tool_call` → `Hello, <name>!`) — via the WP-1206 test host
  (the in-core e2e arrives with WP-1210).
- The TS original still green through the sidecar (A/B baseline).

## Out of scope
- The other five ports (WP-1212–1216).
