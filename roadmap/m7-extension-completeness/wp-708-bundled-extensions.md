# WP-708 — Bundled extensions (mcp, codemode, llama, tool-search)

**Milestone:** M7 · **Size:** S · **Status:** open · **Depends on:** 702

## Goal
Per upstream bundled extension (`src/extensions/{mcp,codemode,llama,tool-search}`):
does it load unmodified on pi-rust (it's just an extension!), what core
capability does it need, and what's the effort to enable. One decision row
per extension.

## Acceptance
- Table in-file: per extension — loads? (matrix run), missing core
  capability, enable-WP or non-goal. No code.

## Out of scope
- Implementing their features (separate WPs if approved).
