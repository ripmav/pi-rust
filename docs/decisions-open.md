# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D17 | open | Fate of `oauth`/`auth`-configured MCP servers after E3S5/WP-1405 (D13, answered by the PO 2026-11-24: "core owns all MCP, sidecar hosts none" — D13's old oauth/auth sidecar exception is gone): the core skips such entries with a logged warning (it cannot authenticate them yet) and the sidecar no longer hosts any MCP servers (E3S5 removed its built-in mcp extension + `mcpConfig` path) — such servers now have **no host at all** (before E3S5 the sidecar connected them while alive). | (1) core implements MCP OAuth/`auth` support (the core becomes the sole authenticator); (2) keep the typed skip (entries stay inert, warning documents it); (3) reject such entries with a hard error at load (fail fast instead of warn + inert) | (2) status quo — the pre-existing typed-skip behavior, now with no host | MCP feature planning |
