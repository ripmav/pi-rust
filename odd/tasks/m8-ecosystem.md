# M8 — Ecosystem (feature doc)

Upstream pin: `earendil-works/pi @ 085c84e` (unchanged from M3–M7; no bump recorded for M8).
House rules (inherited): one GitHub stack per milestone; first WP PR based on `main`; subsequent WP PRs stacked on the previous WP head; one PR per WP; branch `feat/wp-NNN-<slug>`; each PR links its WP issue + one `type:*` label.

## Specs
S1 (WP-801 acceptance): "Fixture skill discovered; frontmatter validated (name/description); model prompt contains the skill block per upstream format (golden); invalid skill skipped with warning."
S2 (WP-802 acceptance): "Upstream RPC client (from the pinned commit) drives pi-rust for: init, prompt, tool-result, quit; protocol diff vs. upstream empty for the implemented commands."
S3 (WP-803 acceptance): "Memo in-file; no code."
S4 (WP-804 acceptance): "Memo in-file; no code."

## Tasks
- T1 / WP-801 — Skills system (discovery, frontmatter, system-prompt injection, headless+TUI command) — S1 — issue #92 — branch `feat/wp-801-skills` — commit: PENDING
- T2 / WP-802 — RPC mode parity (upstream `--mode rpc` surface) — S2 — issue #93 — branch `feat/wp-802-rpc-parity` — commit: (pending)
- T3 / WP-803 — Extension packages (npm) evaluation memo — S3 — issue #94 — branch `feat/wp-803-packages-eval` — commit: (pending)
- T4 / WP-804 — Session export (HTML) evaluation memo — S4 — issue #95 — branch `feat/wp-804-export-html-eval` — commit: (pending)

## Log
L1 (2026-11-12): M8 setup — roadmap M3–M7 status updated to done (M7: stack #3461256, PRs #83–#91, pending merge); first WP branch `feat/wp-801-skills` from `main` (cdd1400). M8 scope: WP-801 Skills (M), WP-802 RPC parity (M), WP-803 packages eval (S), WP-804 export-html eval (S). Order: 801 → 802 → 803 → 804 (evals last: docs-only).

L2 (WP-801 design, 2026-11-13): skills ported to a new `src/skills.rs` module, upstream `core/skills.ts` @ 085c84e semantics:
- Discovery: `<agentDir>/skills` + `<cwd>/.pi/skills`; dir scan rules verbatim (dir with SKILL.md = skill root, no deeper recursion; else direct root `.md` children + recurse subdirs for SKILL.md; skip dot-dirs and node_modules).
- Frontmatter: upstream `---`-delimited extraction verbatim; v0 minimal scalar parser (`key: value`, optional matching quotes, booleans) for `name`/`description`/`disable-model-invocation` only; unparseable declared skill = warning `failed to parse skill file` (fixed message; upstream surfaces the yaml error text — recorded deviation). Block scalars/multiline values = parse error (recorded deviation; upstream uses the yaml lib).
- Validation: upstream-exact messages (name >64 / charset / hyphen edges / `--`; description required / >1024 with count); name falls back to parent dir name; skill loads WITH name warnings but is skipped without a description; plain root `.md` without description = silent skip; first-loaded-wins collision + `name "X" collision` warning.
- Prompt: `formatSkillsForPrompt` verbatim (reader hint = `read` — pi-rust always has the read tool), `disable-model-invocation` skills excluded from the prompt, XML-escaped name/description/location, no truncation; appended to the core system prompt as the upstream `<skills>` section (`<skills>\n{block.trim()}\n</skills>`, `\n\n`-separated) BEFORE the `before_agent_start` hook (so handlers see it, replacement replaces the whole prompt — upstream parity).
- `/skill:<name> [args]` expansion (upstream `agent-session.ts:2299` verbatim): `<skill name="N" location="P">\nReferences are relative to {baseDir}.\n\n{body.trim()}\n</skill>` (+`\n\n{args}`); unknown skill passes through unchanged; TUI dispatch arm (parse_invocation already yields `skill:foo` as the name) + headless positional-prompt expansion (upstream expands core-side in all modes); read failure = corelog warning + pass-through.
- v0 non-goals (upstream has, pi-rust doesn't): `.agents/skills` ancestor + `~/.agents/skills` walk, settings `skills` array, CLI `--skill` paths, extension `resources_discover`, .gitignore/.ignore filtering, symlink real-path dedupe, TUI collapsible skill-block rendering (plain transcript text instead), `/skill:` entries in the slash menu (dispatchable only), `pi skills list` CLI.
- Tests: in-crate goldens (format byte-exact vs upstream, discovery, validation, expansion), in-crate TuiApp dispatch test, e2e `tests/skills.rs` (discovery + invalid-skill stderr warning + clean run).

L3 (WP-801 evidence, 2026-11-13): new `src/skills.rs` (discovery, minimal frontmatter, upstream-exact validation messages, `formatSkillsForPrompt` + `<skills>` section goldens, `/skill:` expansion) — 13 in-crate tests (RED observed: 4 failures on first run — golden indentation, `parentDirName` fallback for root `.md`, kept invalid frontmatter name — all reconciled to upstream semantics). Wiring: `Core.skills` + `append_section` into the system prompt before `before_agent_start` (agent.rs), discovery + warning corelog + headless `/skill:` prompt expansion + TUI `with_skills` + dispatch arm (cli.rs, tui/mod.rs). TDD: in-crate RED→GREEN; TUI dispatch test + 3 e2e in `tests/skills.rs` (system-prompt golden via before_agent_start capture extension, invalid-skill stderr warning, headless expansion golden incl. unknown-skill pass-through). Full suite at WP-801 head (branch from pre-M7 main cdd1400): 240 in-crate (226 baseline + 14 new), 23 e2e (main set), 3 PTY, 3 skills e2e, 43 smoke — all green. README: new "Skills (M8, WP-801)" section. Commit: PENDING.
