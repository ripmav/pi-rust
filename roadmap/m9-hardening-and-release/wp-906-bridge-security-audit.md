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

## Audit memo (2026-11-14, el Gentleman, read-only pass + fixes)

Scope per WP: bridge input validation, extension exec surface, path
traversal in tool handlers, secret redaction in all logs.

### Verified clean
- **Bridge input validation (M1, re-verified)**: `sidecar/bridge.mjs`
  enforces `MAX_LINE` = 1 MiB per command line — complete over-long
  lines are rejected with a parse-error response and the stream
  continues; an unterminated buffer past the cap is reset (no
  unbounded accumulation, no line-swallowing). Unknown/typed
  commands fail closed (`Unknown command`, no default execution).
- **Extension exec surface**: the sidecar never shells out. The
  command surface (`command`, `shortcut`, `extension_tool_call`,
  `tool_call`) resolves only to *registered* extension handlers —
  no eval, no dynamic code, no `child_process` in `sidecar/*.mjs`.
  Extension code itself runs with full node access **by design**
  (upstream parity: extensions are trusted user-installed code);
  that is a trust decision, not a bridge defect.
- **Nested tool calls** (extension `executeTool` → core
  `tool_call_request`) execute through the Rust tool handlers —
  the audited surface, not a second one.
- **Secrets on the wire**: `init` carries protocol version, UI
  methods, MCP config and keybindings only — no API keys, no
  provider URLs. Provider keys live only in the Rust core
  (sidecar never touches model calls — architectural guarantee).
- **Sidecar spawn**: `tokio::process::Command` with an argv vector
  (no shell) — `--extension-dir` values cannot inject.
- **Error redaction**: WP-904 maps provider failures to
  redacted messages (no key, no URL, test-enforced incl. a
  creds-bearing base URL). Core log never logs model/provider
  config, URLs or keys (verified by grep over all `corelog::line`
  call sites).
- **Tool path handling** (`read`/`write`/`edit`/`bash`): arbitrary
  path access is **upstream parity by design** (the coding agent
  operates on the user's filesystem; upstream pi has the same
  semantics, `@`/`~`/relative resolution included). No pi-rust
  regression found.

### Findings + dispositions
| # | Sev | Finding | Disposition |
|---|-----|---------|-------------|
| 1 | MED | Bash full-output log in the shared temp dir was created with default 0644 — complete command output (which may echo secrets) was world-readable on multi-user hosts. | **Fixed this WP**: `OpenOptions::mode(0o600)` (unix), test `bash_capture_full_log_is_owner_only`. |
| 2 | MED | `agent_dir` was created with default 0755 — it holds `auth.json` (API keys) and session transcripts. | **Fixed this WP**: `ensure_agent_dir` sets 0700 when pi-rust creates the dir; pre-existing dirs keep their mode. Tests `ensure_agent_dir_new_is_owner_only` / `_existing_keeps_mode`. |
| 3 | LOW | No line-length cap on the **sidecar→core** direction (core→sidecar has the 1 MiB cap). A buggy/malicious extension could push an oversized line into core memory. | Documented, not fixed: an extension already holds full node access in the sidecar (weaker bounds), so this is not a privilege escalation; the bridge is internal to one user's own processes. |
| 4 | LOW | `install.sh` downloads release tarballs over TLS but verifies no signatures (WP-903 "signed-ish" v0). | Documented: trust boundary = GitHub supply chain + TLS, same as the large majority of install scripts. Signing is a post-M9 hardening item. |
| 5 | LOW | `BashCapture` temp files are removed only via `finish()` — an aborted run (panic) leaks the file. | Documented: pre-existing, bounded (uuid-named, auto-cleaned by the OS tmp reaper), no secret-exposure delta beyond finding 1 (now 0600). |

**Zero unaddressed HIGH findings** (none found).

### Native review (high-risk scope, per WP acceptance)
- Candidate: the 906 fix commit on `feat/wp-906-security-audit`.
- Lineage: (recorded after the native review run)
