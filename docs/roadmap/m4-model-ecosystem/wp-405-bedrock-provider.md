# WP-405 — AWS Bedrock provider

**Milestone:** M4 · **Size:** M · **Status:** open · **Depends on:** 302, 303

## Goal
Bedrock driver: SigV4-signed requests (AWS SDK or manual sigv4),
`converse-stream` event mapping, profile/region from env
(`AWS_PROFILE`, `AWS_REGION`).

## Acceptance
- Mock tests: sigv4 signature presence + canonical shape (deterministic
  vectors), event stream assembly (text + toolUse); auth-failure mapping.

## Out of scope
- OAuth/SSO credential flows (env/profile keys only).
