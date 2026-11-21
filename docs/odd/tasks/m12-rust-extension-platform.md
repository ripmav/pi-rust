# M12 — Rust extension platform (Epic E1)

Implementation plan for [Epic E1](../../epics.md) (native Rust extension
interface & framework). Design contract: [ADR 0001](../../adr/0001-rust-extensions-as-bridge-subprocesses.md)–[ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md),
target state in [architecture.md](../../architecture.md). One file per task
under
[roadmap/m12-rust-extension-platform/](../../roadmap/m12-rust-extension-platform/)
(WP-1201–1217, repo convention).

## Specs

- S1 (epic requirement, user 2026-11-21): Rust extension interface beside the
  sidecar; new extensions in Rust, old ones ported; framework for easy
  creating extensions. The sidecar stays (TS path unchanged).
- S2 (design): ADRs 0001–0006 are binding — user extensions = bridge-v6
  subprocesses (isolation), full 1:1 API mirror, `pi-ext` workspace crate,
  shared discovery + WP-1102 trust, six ports as core extensions, core
  extensions in-process `.so`.
- S3 (no regression): the sidecar, bridge protocol and the existing
  extension e2e matrix stay green throughout; every port must pass the same
  e2e scenario as its unmodified original.
- S4 (process): every WP = its own branch/PR (WPs stack in the listed
  order; `stacked-prs` skill). Story `Status` in [epics.md](../../epics.md)
  is updated in the same PR (po rule 1). Branch naming:
  `feat/wp-12<n>-<slug>`.

## Tasks

Build order top-down; each task = one WP file (Goal / Scope / Acceptance) =
one PR.

### Phase 1 — `pi-ext` framework crate (E1S2)

- T1 / [WP-1201](../../roadmap/m12-rust-extension-platform/wp-1201-workspace-pi-ext-skeleton.md) — workspace + `pi-ext` skeleton — **merged into main as `bb92239` (2026-11-21), CI 7/7 green (run 44948477220)**
- T2 / [WP-1202](../../roadmap/m12-rust-extension-platform/wp-1202-pi-ext-wire-types.md) — `pi-ext`: bridge wire types — **merged into main as `661680e` (2026-11-21), CI 7/7 green (run 44954140387)**
- T3 / [WP-1203](../../roadmap/m12-rust-extension-platform/wp-1203-pi-ext-api-core.md) — `pi-ext`: API surface — core — **in progress** (branch `feat/wp-1203-pi-ext-api-core`)
- T4 / [WP-1204](../../roadmap/m12-rust-extension-platform/wp-1204-pi-ext-api-parity.md) — `pi-ext`: API surface — rest + parity — **open**
- T5 / [WP-1205](../../roadmap/m12-rust-extension-platform/wp-1205-pi-ext-user-target.md) — `pi-ext`: user target (v6 process) — **open**
- T6 / [WP-1206](../../roadmap/m12-rust-extension-platform/wp-1206-pi-ext-core-target.md) — `pi-ext`: core target (C ABI + dlopen) — **open**

### Phase 2 — core runtime (E1S3)

- T7 / [WP-1207](../../roadmap/m12-rust-extension-platform/wp-1207-n-peer-runtime.md) — N-peer runtime in the core — **open**
- T8 / [WP-1208](../../roadmap/m12-rust-extension-platform/wp-1208-discovery-v7-trust.md) — core-owned discovery + protocol v7 + trust — **open**
- T9 / [WP-1209](../../roadmap/m12-rust-extension-platform/wp-1209-port-hello.md) — port `hello` (first core extension) — **open**
- T10 / [WP-1210](../../roadmap/m12-rust-extension-platform/wp-1210-core-ext-loading.md) — core-extension loading in the core — **open**
- T11 / [WP-1211](../../roadmap/m12-rust-extension-platform/wp-1211-mixed-session-bench.md) — mixed session + bench re-check — **open**

### Phase 3 — remaining ports (E1S4)

Each: same e2e scenario as the unmodified original (same fixtures), in-core.

- T12 / [WP-1212](../../roadmap/m12-rust-extension-platform/wp-1212-port-permission-gate.md) — port `permission-gate` — **open**
- T13 / [WP-1213](../../roadmap/m12-rust-extension-platform/wp-1213-port-claude-rules.md) — port `claude-rules` — **open**
- T14 / [WP-1214](../../roadmap/m12-rust-extension-platform/wp-1214-port-prompt-customizer.md) — port `prompt-customizer` — **open**
- T15 / [WP-1215](../../roadmap/m12-rust-extension-platform/wp-1215-port-with-deps.md) — port `with-deps` (own external dependency) — **open**
- T16 / [WP-1216](../../roadmap/m12-rust-extension-platform/wp-1216-port-plan-mode-guide.md) — port `plan-mode` + porting guide — **open**

### Phase 4 — authoring framework (E1S5)

- T17 / [WP-1217](../../roadmap/m12-rust-extension-platform/wp-1217-scaffolding-docs.md) — scaffolding + authoring docs — **open**

## Log

- **L1** (2026-11-21): Epic E1 + ADRs 0001–0006 + `docs/architecture.md`
  recorded (PR #150); E1S1 done. Task list T1–T17 written.
- **L2** (2026-11-21): Tasks split into one WP file each under
  `docs/roadmap/m12-rust-extension-platform/` (WP-1201–1217, M11 convention);
  this file is now the index. Next: WP-1201.
- **L3** (2026-11-21): WP-1201 delivered — PR #151 (stack #152, stacked on
  the E1 docs PR #150) merged into main as `bb92239`, CI 7/7 green
  (run 44948477220); E1S2 at 1/6, next: WP-1202.
- **L4** (2026-11-21): WP-1202 delivered — PR #153 merged into main as
  `661680e`, CI 7/7 green (run 44954140387); E1S2 at 2/6, next: WP-1203.
