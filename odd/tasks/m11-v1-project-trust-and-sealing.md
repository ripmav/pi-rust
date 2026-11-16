# M11 — v1: project trust & sealing

Rust port of earendil-works/pi: milestone M11 — pin cycle, project trust
model, project `.pi/` config under the gate, v1.0 sealing & release.
Upstream pin: `earendil-works/pi @ 17d6ca759a070c377147f6d76b72dab9a3ba42a3`
(at M11 start; WP-1101 bumps to `64a15f4`).

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
- T2 / WP-1101 — Upstream pin refresh (17d6ca7 → 64a15f4) — S2 — branch `feat/wp-1101-upstream-pin-refresh` — PR: — commit: (see L3)
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
