# WP-906 — Bridge security audit

**Milestone:** M9 · **Size:** S · **Status:** open · **Depends on:** M7

## Goal
Independent security pass over the sidecar bridge: input validation (line
limits done in M1 — re-verify), command injection surface (extension
`exec`), path traversal in tool handlers, secret redaction in all logs.
Output: findings list + fix WPs.

## Acceptance
- Audit memo + native-review lineage (high-risk scope) in-file; zero
  unaddressed HIGH findings.

## Out of scope
- External penetration testing.
