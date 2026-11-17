# M11 — v1: project trust & sealing

Rust port of earendil-works/pi: milestone M11 — pin cycle, project trust
model, project `.pi/` config under the gate, v1.0 sealing & release.
Upstream pin: `earendil-works/pi @ 64a15f4bee63d761d5398bf4126ea63abef0dc98`
(WP-1101 bump 2026-11-16; was `17d6ca759a070c377147f6d76b72dab9a3ba42a3` at M11 start).

## Specs
- S1 (M11 start): "M11 mit Pin-Zyklus starten (empfohlen)" — user-selected
  next step after M10 close (2026-11-16): M11-Planung (Milestones-Doku +
  WP-Set) und dann WP-1101 = Upstream-Pin-Refresh sofort, exakt das M10-
  Pattern (WP-1001 zuerst).
- S2 (WP-1101 acceptance): pin `64a15f4bee63d761d5398bf4126ea63abef0dc98`;
  all pin references updated and consistent; export matrix re-verified;
  matrix + smoke + full suite green on the new pin; bench baseline
  re-recorded (3-sample median).
- S3 (WP-1102 acceptance): untrusted `<cwd>/.pi/` content has zero
  behavioral effect; granting trust is an explicit, persistent,
  user-visible action; trust is per-path.
- S4 (WP-1103 acceptance): trusted project settings + MCP from `.pi/`
  applied; untrusted project zero effect.
- S5 (WP-1104 acceptance): stability contract committed; CHANGELOG covers
  every release since v0.0.0; v1.0.0 published, acceptance check green
  (`--version` → `pi-rust 1.0.0`), M11 marked delivered.

## Tasks
- T1 / M11 setup — feature doc + todo + branch — S1 — branch `feat/wp-1101-upstream-pin-refresh` — commit: (see L2)
- T2 / WP-1101 — Upstream pin refresh (17d6ca7 → 64a15f4) — S2 — branch `feat/wp-1101-upstream-pin-refresh` — PR #146 — **merged into main as `c3c5a91` (2026-11-16), CI 7/7 green (run 44158531958)** — commits `a4cc8ee` (pin) + `ec9057c` (M11 setup); native review APPROVED (`review-119f0170c6991780`, risk+reliability, authority burned)
- T3 / WP-1102 — Project trust model (design + implementation) — S3 — branch: — PR: — commit:
- T4 / WP-1103 — Project `.pi/` config under the gate — S4 — branch: — PR: — commit:
- T5 / WP-1104 — v1.0 sealing & release — S5 — branch: — PR: — commit:

## Log
- **L1** (2026-11-16): M10 endgültig geschlossen (v0.2.1 released, WP-1007
  review per operator decision gestrichen, 0 offene Items). User decision
  for next step: "M11 mit Pin-Zyklus starten (empfohlen)" — M11-Planung
  (Milestones-Doku + WP-Set: 1101 pin, 1102 trust, 1103 project .pi/
  config, 1104 v1.0 sealing) und WP-1101 sofort, exakt das M10-Pattern.
- **L2** (2026-11-16): M11 setup — `roadmap/01-milestones.md` M11 row +
  dependency line added; `roadmap/m11-v1-project-trust-and-sealing/`
  (WP-1101..1104); this feature doc; WP-1101 upstream delta explored
  (consumed packages: coding-agent/tui/mcp unchanged, ai +9-3 Mistral
  strict-mode fix — not a gap, pi-rust sends no strict field; undici
  8.12.4 dep bump rides along; p2/p3/chord/durable = v1 non-goals).
- **L3** (2026-11-16): T2/WP-1101 — pin bumped 17d6ca7 →
  64a15f4bee63d761d5398bf4126ea63abef0dc98 (origin/main, +73). Consumed
  packages: coding-agent/tui/mcp unchanged, ai = Mistral strict-mode
  opt-in fix (no pi-rust impact — no `strict` field in src/provider),
  lockfile diff = undici 8.12.4 only. Export matrix: 157/87/93/27
  (coding-agent/ai/tui/mcp), 0 added / 0 removed. Suite: 341 passed / 0
  failed (282 in-crate, 2 auth PTY, 1 cli-help, 45 e2e matrix, 4 rpc,
  3 skills, 4 tui PTY); smoke 74/0; fmt + clippy clean. Bench re-recorded
  (3-sample median: 1196.6 ms / 5.4 MB / 5429.8 events/s). Zero code
  migration. S2 met. Note: Cargo.lock auto-synced 0.2.0 → 0.2.1 by cargo
  (pre-existing drift from the v0.2.1 release — Cargo.toml already 0.2.1).
- **L5** (2026-11-16): WP-1101 merged into main as `c3c5a91` (PR #146,
  CI 7/7 green incl. bench baseline compare + windows-latest). M11 is at
  1/4 WPs; next: WP-1102 project trust model (design phase). Note: a
  stale M7-era stash (`a7bc0fb`) was accidentally popped during the
  post-merge doc commit and conflicted on src/tui/{mod,terminal}.rs;
  resolved by restoring HEAD content (the stash-side content was an old
  M7 relic), stash entries left intact in `git stash list`.
- **L4** (2026-11-16): Native review for the WP-1101 commit range:
  assess flagged the range high-risk (ci.yml `UPSTREAM_SHA` = shell_source)
  with reviewDue=true; the RDD pre-delivery guard then blocked PR
  creation. User decision: attempt the native review (consent granted in
  UI). Lineage `review-119f0170c6991780` (tier high, lenses
  risk+reliability): both reviewers COMPLETED — review-risk 91.3 s,
  review-reliability 248.5 s, both stopReason `stop` (the solheim
  provider defect that killed the WP-1007 candidate at 1045 lines did
  not recur at 304 lines). Outcome **approved**, acknowledgement
  committed, authority burned (consumed revision sha256:92368b72…).
  PR #146 created via GitHub REST API (the RDD guard's `gh pr create`
  repository resolution fails in this environment; git push and the REST
  API both work). CI on PR #146 running.
