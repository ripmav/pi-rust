# WP-404 — Cloudflare providers (gateway + workers-ai)

**Milestone:** M4 · **Size:** S · **Status:** open · **Depends on:** 401

## Goal
cloudflare-ai-gateway (OpenAI-compat path + account routing) and
cloudflare-workers-ai (native invoke) drivers.

## Acceptance
- Mock tests per sub-provider: request URL/headers per upstream
  `cloudflare-*.ts`; stream assembly.

## Out of scope
- Cloudflare auth flow beyond env keys/tokens.
