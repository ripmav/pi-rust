# WP-403 — Azure OpenAI provider

**Milestone:** M4 · **Size:** S · **Status:** open · **Depends on:** 401

## Goal
Azure variant: `api-version` query/header, `api-key` header (not Bearer),
deployment-as-model semantics.

## Acceptance
- Mock tests: header/query shape; deployment name passes through as model.

## Out of scope
- Azure auth via Entra ID tokens (env key only).
