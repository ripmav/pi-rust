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
- S3 (WP-303): key resolution in verified upstream order (`resolve.ts`: a
  stored credential owns the provider): override → `auth.json` → ambient env
  → clear error naming the exact env var; key material never appears in logs
  or error text. v0: literal keys + `${ENV}` refs only, no command execution.
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
| T3 | S3 | 303 | #13 | inline TDD | 386aa6e |
| T4 | S4 | 304 | #14 | inline TDD | 9bb9352 |
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
- L5: Slice boundary for M3 delivery: one PR `feat/m3-config-and-state` →
  `main` (base main is only correct after PR #10 lands M2 in main; the
  milestone slice = commits since `feat/extension-bridge`, not since main).
- L6: T2/WP-302 done (b2e00cf): `src/registry.rs` + `build.rs` — the 43
  upstream provider data files merged at build time into OUT_DIR (zero new
  deps, frozen to the pin); `ModelRegistry::bundled()` (OnceLock),
  `resolve(provider, id)`, `providers()`, `models_of()`, `count()`. Dedup per
  `provider/id` (upstream lists one model under several api groups — api
  variants matter only for M4 driver dispatch). Drift test compares the baked
  catalog against the data dir (unique-key count). Capability surface:
  reasoning/thinking, input modalities, context window, max tokens, cost — a
  `supports_tools` query is a consumer concern of WP-406 (recorded).
  Full suite: 15 in-crate + 7 e2e.
- L7: S3 correction (verified in upstream `ai/src/auth/resolve.ts:26-28,68-87`):
  a STORED auth.json credential wins over ambient env; env is consulted only
  when nothing is stored. WP-303 file + this spec updated.
- L8: Merged origin/main into feat/m3-config-and-state (d1b71cb) to carry the
  roadmap (PR #9) into the branch — the branch was cut from
  feat/extension-bridge, which predates the roadmap. Consequence: the M3 PR
  (main ← feat/m3-config-and-state) contains M2 as well; after the M3 PR
  merges, PR #10 (M2 → main) is superseded and can be closed.
- L9: T3/WP-303 done (386aa6e): `src/auth.rs` — resolve_api_key(provider,
  agent_dir): stored auth.json api_key credential (literal or ${ENV} ref
  with credential-env overlay) > ambient env (per-provider table, subset of
  upstream env-api-keys) > typed error naming the env var. v0 deviations
  recorded in the WP file: no command-ref execution, oauth = typed error.
  build_provider now takes agent_dir (M2 env behavior = the fallback step).
  Security pin: redaction test (no key material in errors). 8 auth unit
  tests, each using its own env var (no parallel-test races). Suite: 22
  in-crate + 7 e2e.
- L10: T4/WP-304 done (9bb9352): `src/session.rs` rewritten — session files
  now in the upstream JSONL v3 format: header
  ({type:"session",version:3,id,timestamp,cwd}), entries
  (message/session_info/custom/custom_message/thinking_level_change/
  model_change) with id+parentId tree, ISO-8601 timestamps, pi-ai message
  shapes (toolCall `input`). `Session::create` / `Session::load` (leaf-chain
  walk — branches handled structurally). Default path = upstream layout
  `<agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl` (encode_cwd =
  upstream `getDefaultSessionDirPath`). **Cross-compat proven by test:**
  the original upstream `SessionManager.open()` (built dist) loads a
  pi-rust-written session (header version, entry count, branch roles).
  Reverse direction: golden hand-written upstream-shaped file loads in
  pi-rust. `set_model`/`set_thinking_level` requests now persist
  model_change/thinking_level_change entries. v0 files: typed migration
  error (no implicit migration — decision recorded). Assistant usage fields
  stay zero until WP-407 captures real usage. Deps added: chrono (ISO
  timestamps). Suite: 27 in-crate + 8 e2e + 42 smoke.
