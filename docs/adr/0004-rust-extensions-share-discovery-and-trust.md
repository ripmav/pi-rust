# 4. Discover Rust extensions in the existing extension locations under the same trust gate

Date: 2026-11-21

## Status

Accepted

## Context

WP-1102 established the trust model: untrusted project-scope content
(`<cwd>/.pi/`) has zero behavioral effect; granting trust is explicit,
persistent, user-visible and per-path; the sidecar excludes untrusted
project-scope TS extensions. Rust extensions are loaded from disk by the same
core and face the same threat model (a malicious project repo shipping an
extension that would auto-load). A separate discovery configuration for Rust
extensions would duplicate path handling, **fork the trust logic** (a security
boundary), and force authors to know two places. This ADR settles D4.

## Decision

Rust extensions are discovered from the **same locations** as TS extensions:
the agent dir (`PI_CODING_AGENT_DIR`), `<cwd>/.pi/extensions` (project scope),
and `--extension-dir` entries. The on-disk form is distinguished by a **marker
for the Rust kind** (subdirectory or platform binary — pinned in E1S1's
architecture doc so the two kinds cannot collide within one directory).

Trust rules are **identical**: project-scope Rust extensions are inert unless
their path is trusted; agent-dir and CLI-scope Rust extensions load
unconditionally. There is no separate configuration for Rust extensions.

## Consequences

- The trust gate is reused, not forked: one gate, two extension kinds. E1S3's
  acceptance ("project-scope Rust extensions are gated by the trust model") is
  testable against the existing WP-1102 test patterns.
- Authors have one mental model — "extensions live in extension dirs" —
  regardless of kind.
- The core must classify each candidate (TS → sidecar, Rust → spawn) before
  dispatch: a small routing step in E1S3.
- The architecture must define the marker (E1S1) so a Rust extension next to
  TS files in the same directory cannot be misloaded by either runtime.
