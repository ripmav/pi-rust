# WP-401 — OpenAI-compatible provider families

**Milestone:** M4 · **Size:** S · **Status:** open · **Depends on:** M2

## Goal
One code path, many providers: groq, xai, cerebras, deepseek, minimax,
mistral (compat mode), openrouter, together, fireworks, huggingface, ollama,
custom `--provider openai --base-url …`.

## Scope
- Provider-id → (family, default base URL, key env) table; the M2 OpenAI
  driver already speaks the wire format — this WP is the table + validation.

## Acceptance
- Table test: each id resolves to family openai + correct defaults; a
  mock-server test per family with a non-default base URL (one shared
  harness).

## Out of scope
- Family-specific quirks (only add when a real provider misbehaves, then as
  a named sub-task).
