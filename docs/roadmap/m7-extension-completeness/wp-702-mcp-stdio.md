# WP-702 — MCP transport (stdio)

**Milestone:** M7 · **Size:** L · **Status:** open · **Depends on:** 301

## Goal
`registerMcpServer` (and config-declared servers, `mcp-servers.ts` parity)
actually connect: MCP client over stdio (spawn, JSON-RPC 2.0,
initialize/tools/list/tools/call), MCP tools appear as extension-style
tools with upstream naming.

## Scope (split at start)
- 702a: minimal MCP client (stdio, JSON-RPC) + tests against a fixture
  MCP server.
- 702b: tool bridging: MCP tools → agent loop dispatch + result mapping.
- 702c: config: server entries from `~/.pi`/project config (WP-301).

## Acceptance
- Fixture server: tool list + call round-trip; crash/restart semantics per
  upstream; unmodified MCP-using extension from upstream examples works.

## Out of scope
- HTTP/SSE transport (WP-703), OAuth for remote MCP.
