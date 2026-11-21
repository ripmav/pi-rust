# WP-603 — Streaming display

**Milestone:** M6 · **Size:** S · **Status:** open · **Depends on:** 601

## Goal
Tokens render as they arrive (provider streams already exist from M2);
tool-call deltas show a pending indicator; no full-screen re-render per
token (perf: batch at 30–60fps).

## Acceptance
- Latency test: first token visible < 100ms after mock-stream first chunk;
  CPU/frame-budget sanity on a 100-token stream.

## Out of scope
- Partial-markdown reflow beyond per-line.
