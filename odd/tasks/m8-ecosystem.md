# M8 — Ecosystem (feature doc)

Upstream pin: `earendil-works/pi @ 085c84e` (unchanged from M3–M7; no bump recorded for M8).
House rules (inherited): one GitHub stack per milestone; first WP PR based on `main`; subsequent WP PRs stacked on the previous WP head; one PR per WP; branch `feat/wp-NNN-<slug>`; each PR links its WP issue + one `type:*` label.

## Specs
S1 (WP-801 acceptance): "Fixture skill discovered; frontmatter validated (name/description); model prompt contains the skill block per upstream format (golden); invalid skill skipped with warning."
S2 (WP-802 acceptance): "Upstream RPC client (from the pinned commit) drives pi-rust for: init, prompt, tool-result, quit; protocol diff vs. upstream empty for the implemented commands."
S3 (WP-803 acceptance): "Memo in-file; no code."
S4 (WP-804 acceptance): "Memo in-file; no code."

## Tasks
- T1 / WP-801 — Skills system (discovery, frontmatter, system-prompt injection, headless+TUI command) — S1 — issue #92 — branch `feat/wp-801-skills` — commit: (pending)
- T2 / WP-802 — RPC mode parity (upstream `--mode rpc` surface) — S2 — issue #93 — branch `feat/wp-802-rpc-parity` — commit: (pending)
- T3 / WP-803 — Extension packages (npm) evaluation memo — S3 — issue #94 — branch `feat/wp-803-packages-eval` — commit: (pending)
- T4 / WP-804 — Session export (HTML) evaluation memo — S4 — issue #95 — branch `feat/wp-804-export-html-eval` — commit: (pending)

## Log
L1 (2026-11-12): M8 setup — roadmap M3–M7 status updated to done (M7: stack #3461256, PRs #83–#91, pending merge); first WP branch `feat/wp-801-skills` from `main` (cdd1400). M8 scope: WP-801 Skills (M), WP-802 RPC parity (M), WP-803 packages eval (S), WP-804 export-html eval (S). Order: 801 → 802 → 803 → 804 (evals last: docs-only).
