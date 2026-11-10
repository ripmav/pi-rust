# WP-402 — Google / Gemini provider (+ Vertex)

**Milestone:** M4 · **Size:** M · **Status:** open · **Depends on:** 302, 303

## Goal
Native `generateContent`/streaming driver for google (+ google-vertex
variant), incl. function-call mapping to/from pi tool calls.

## Acceptance
- Mock-server tests: request shape (system, tools, history mapping), SSE
  stream assembly (text + functionCall), non-2xx mapping; matrix re-run.

## Out of scope
- Code execution / URL context features.
