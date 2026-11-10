# WP-802 — RPC mode parity

**Milestone:** M8 · **Size:** M · **Status:** open · **Depends on:** M6

## Goal
pi-rust exposes upstream's `--mode rpc` surface (the protocol the bridge
reuses), so external drivers can control pi-rust exactly like upstream pi.

## Acceptance
- Upstream RPC client (from the pinned commit) drives pi-rust for: init,
  prompt, tool-result, quit; protocol diff vs. upstream empty for the
  implemented commands.

## Out of scope
- RPC features added upstream after the pin (revisit at pin bumps).
