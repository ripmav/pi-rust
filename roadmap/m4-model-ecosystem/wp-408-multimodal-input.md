# WP-408 — Image / multimodal input

**Milestone:** M4 · **Size:** M · **Status:** open · **Depends on:** 302

## Goal
Users can attach images (`--image <path>` headless; TUI paste later) and the
provider drivers emit image content blocks for capable models; non-capable
model → clear error.

## Acceptance
- openai + anthropic image-block request-shape tests; capability gate test
  (registry); file errors (missing/not-an-image) typed.

## Out of scope
- Model-generated images; audio.
