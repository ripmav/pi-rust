# Milestone 3: `~/.pi` & session state

A pi user's existing setup keeps working: agent config, model registry, auth
storage, and **upstream-compatible session files** — the foundation the TUI
(M6) builds on. Upstream pin: `earendil-works/pi @ 085c84e` (kept for M3).

## Specs

Canonical acceptance criteria live in the roadmap WP files
(`roadmap/m3-config-and-state/wp-30*.md`), merged via PR #9 (approved plan).
Summarized per task below; where a WP file states exact behavior (formats,
order, errors), the WP file wins.

- S1 (WP-301): core loads agent config from `~/.pi/agent` (or `--agent-dir`);
  settings surface = what upstream `config.ts` + `defaults.ts` read; unknown
  keys ignored; malformed config → clear error (exit 1).
- S2 (WP-302): model registry resolves `provider/model` ids from the bundled
  upstream catalog; capability queries (`supports_tools`, `supports_thinking`,
  default `max_tokens`); catalog count matches upstream data at build time.
- S3 (WP-303): key resolution order env → auth storage → clear error naming
  the exact env var; key material never appears in logs/error text.
- S4 (WP-304): session files written AND read in the upstream JSONL format
  (round-trip + golden-file vs a real upstream session); v0 files: `--migrate-v0`
  or left behind (decide at implementation, record here).
- S5 (WP-305): `--list-sessions`, `--resume <id|path>` (history continues the
  conversation), `--fork <id>`.
- S6 (WP-306): default agent dir becomes `~/.pi/agent`; `--isolated` keeps the
  temp-dir behavior (tests/CI); matrix re-run green under `--isolated`.
- S7 (WP-307): prompt templates discovered (project beats agent dir), loaded,
  headless `--template <name> [args]` expands with upstream `$ARGUMENTS`
  semantics; unknown template → error listing available.

## Tasks

| ID | Spec | WP | Issue | Route | Commit |
|----|------|----|-------|-------|--------|
| T1 | S1 | 301 | #11 | inline TDD | bee3fd7 |
| T2 | S2 | 302 | #12 | inline TDD | — |
| T3 | S3 | 303 | #13 | inline TDD | — |
| T4 | S4 | 304 | #14 | inline TDD | — |
| T5 | S5 | 305 | #15 | inline TDD | — |
| T6 | S6 | 306 | #16 | inline TDD | — |
| T7 | S7 | 307 | #17 | inline TDD | — |

## Log

- L1: user (verbatim): "merged; Start with M3" — after merging PRs #2/#4/#9;
  M2 was found not yet in main (PR #4 merged into its stacked base), so
  PR #10 (`feat/extension-bridge` → `main`) carries M2 + doc commits; M3
  develops on `feat/m3-config-and-state` from `feat/extension-bridge`.
- L2: M3 entry: upstream pin kept at 085c84e (no behavior changes expected;
  revisit at M6 when the TUI needs more surfaces).
- L3: Execution order by dependencies: T1(301) → T2(302) → T3(303);
  T4(304) → T5(305) → T6(306); T7(307) after T1.
- L4: T1/WP-301 done (bee3fd7): `src/config.rs` (AgentConfig from
  `<agent-dir>/settings.json`, camelCase, unknown keys ignored, missing file
  = defaults, malformed = error with path); `resolve_provider_flags`
  (flags > settings > fake; `provider/model` split; bare settings model
  without provider = error until WP-302 registry). CLI: provider/model
  resolve after agent-dir setup; settings `defaultThinkingLevel` seeds the
  core thinking level when present (built-in default stays "off" — M2
  drivers don't send thinking params yet; WP-406 makes the level
  functional). Project `cwd/.pi/settings.json` not read (needs trust model;
  headless v0 scope, recorded in the WP file). RED/GREEN: before wiring
  `--provider openai` without `--model` failed "requires --model"; after,
  settings defaultModel is used (fails on missing key instead);
  `provider/model` settings form resolves the provider. Tests: 4 new config
  unit tests; full suite 18 passed + 42 smoke.
