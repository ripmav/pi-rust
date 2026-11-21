# WP-703 — MCP transport (HTTP/SSE)

**Milestone:** M7 · **Size:** M · **Status:** open · **Depends on:** 702

## Goal
Remote MCP servers over HTTP (+SSE streaming responses), headers/auth from
config (env-only values).

## Acceptance
- Mock HTTP MCP server: initialize + tool call; stream response assembly;
  auth header pinned by test (no secret in logs).

## Out of scope
- Dynamic client registration flows.
