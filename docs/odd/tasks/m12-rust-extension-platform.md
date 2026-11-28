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
- T3 / [WP-1203](../../roadmap/m12-rust-extension-platform/wp-1203-pi-ext-api-core.md) — `pi-ext`: API surface — core — **merged into main as `387d713` (2026-11-21), CI 7/7 green (run 44959071977)**
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
- **L5** (2026-11-21): WP-1203 delivered — PR #154 merged into main as
  `387d713`, CI 7/7 green (run 44959071977; first run's only failure was
  the flaky macOS PTY test `tui_custom_pane_roundtrip`, no pi-ext
  involvement); E1S2 at 3/6, next: WP-1204. Note: GitHub stacks need ≥2
  open PRs, so the single open WP PRs are unregistered until the next
  WP PR opens (register [154, next] then).
- **L6** (2026-11-27): WP-1204 delivered — PR #190
  (`feat/wp-1204-pi-ext-api-parity`): remaining `pi-ext` API surface
  (commands/shortcuts/flags, provider registration + stream surface, MCP
  registration, `ctx.ui` surface incl. `custom` pane + no-UI default,
  `on_mcp_servers_change` + the v6 MCP wire types) and the catalog ↔ API
  parity test (52 capabilities, `pi-ext/tests/parity.rs` ↔
  `fixtures/v6-catalog.jsonl`, both directions); `pi-ext` 0.x section added
  to stability-contract.md; E1S2 at 4/6, next: WP-1205.
- **L7** (2026-11-28): PR #190 bug-review fixes (WP-1204, same branch,
  3 bugs): (1) `get_flag` now coerces by `flag.kind` per upstream
  `applyExtensionFlagValues` — a boolean flag is `true` for any CLI
  occurrence (an `=value` is ignored), a string flag takes the CLI
  string value and a bare occurrence leaves the registered default
  standing; (2) the CLI scan (`cli_flag_value`) now stops at `--`
  (everything after is positional) and is last-write-wins, matching
  upstream `parseArgs`' `unknownFlags` map — was first-match-over-entire-
  argv; (3) `pi_ext::wire::McpServerConfig` widened to the core's
  `register_server` entry shape: added `type` (`stdio`|`http`), stdio
  `cwd` and per-server `timeout` (seconds, number); `auth` stays opaque
  passthrough, now documented as a core rejection (D17) the wire still
  carries; exposure doc corrected to include `hidden`. Pinned by unit
  tests (`pi-ext/src/pi/mod.rs`: flag parse/coercion semantics) and a
  full-upstream-shaped stdio+http entry round-trip fixture
  (`pi-ext/tests/wire.rs`); the v6 catalog fixture is unchanged
  (capability set did not change).
- **L8** (2026-11-28): WP-1205 delivered — PR #191
  (`feat/wp-1205-pi-ext-user-target`, stack #192 on #190): the v6 user
  process runtime — `pi_ext::process::run` turns an `Extension` into a
  bridge-v6-speaking process (stdio NDJSON; `init` handshake with
  `protocolVersion` validation, `apiVersion` answer and the `ready`
  catalog from the stored registrations; load-time MCP forwards as v6
  `mcp_server_register` requests; per-command dispatch tasks — no
  head-of-line blocking, so a dialog or nested `executeTool` may block
  while `extension_ui_response`/`pane_key`/responses keep flowing;
  `provider_stream` ack + `provider_stream_event` lines; panic safety:
  handler code runs in a child task, a panic yields a clean error
  response / protocol default, and a single stdout writer keeps lines
  complete). Example extension `pi-ext-demo` (tools, commands, event
  handlers, flag, shortcut, provider, MCP) + NDJSON e2e harness
  `pi-ext/tests/process.rs` (15 scenarios — ready catalog, tool
  round-trip, decision block, UI dialog round-trip, provider stream,
  parse error, unknown command, panics, shutdown/EOF — no Node sidecar
  involved). Additive: `Extension::events()` (declares `ready.events`;
  Rust has no handler reflection) + a `Sync` bound (concurrent
  dispatch); `ProviderStreamFn`'s emit gains a `Send` bound (the future
  was already `Send + 'a` — the seam was unusable by a stream that
  calls `emit`); `tokio` becomes a `pi-ext` dependency. E1S2 at 5/6,
  next: WP-1206.
