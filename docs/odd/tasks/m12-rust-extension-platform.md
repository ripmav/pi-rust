# M12 — Rust extension platform (Epic E1)

Implementation plan for [Epic E1](../../epics.md) (native Rust extension
interface & framework). Design contract: [ADR 0001](../../adr/0001-rust-extensions-as-bridge-subprocesses.md)–[ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md),
target state in [architecture.md](../../architecture.md).

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
- S4 (process): every task = its own branch/PR (tasks stack in the listed
  order; `stacked-prs` skill). Story `Status` in [epics.md](../../epics.md) is
  updated in the same PR (po rule 1). Branch naming: `feat/e1s<n>-<slug>`.

## Tasks

Build order top-down; each task is small (one PR, independently testable).

### Phase 1 — `pi-ext` framework crate (E1S2)

- **T1 — Workspace + `pi-ext` skeleton.** Repo becomes a Cargo workspace:
  `[workspace]` in the root `Cargo.toml` (root stays the `pi-rust` binary, all
  existing paths unchanged), new empty `pi-ext/` lib crate (0.0.0; deps:
  serde, serde_json). CI builds the workspace; existing jobs unchanged.
  *Acceptance:* `cargo build`/`cargo test` at the root cover both crates;
  full suite + e2e matrix green (no behavior change).
- **T2 — `pi-ext`: bridge wire types.** One source of truth for the v6 wire
  types in `pi-ext`: envelope (command/response/event lines), `init`/`ready`
  payloads, all event payloads of the catalog, tool/command/shortcut/flag/
  provider types. Ported from the hand-rolled `json!` in `src/agent.rs`;
  fixture lines from [bridge-protocol.md](../../bridge-protocol.md).
  *Acceptance:* serde round-trip unit tests against the wire examples in
  bridge-protocol.md; the types compile in the standalone crate (no core
  imports).
- **T3 — `pi-ext`: API surface — core.** `Extension` trait + context:
  `on(event)` for all catalog events, `registerTool` (JSON-schema parameters,
  `exposure`, `defaultActive`), `sendUserMessage`/`appendEntry`, session
  name+label, `exec`, `getSettings`, `setModel`/thinking level
  ([ADR 0002](../../adr/0002-mirror-full-upstream-extension-api.md)).
  *Acceptance:* doc examples compile (`cargo test --doc`); a documented
  "hello tool" example compiles straight from the docs.
- **T4 — `pi-ext`: API surface — rest + parity.** Commands,
  shortcuts/keybindings, flags, provider registration, MCP registration, UI
  surface (`select`/`confirm`/`input`/`editor`/`notify`/`custom`); the
  catalog ↔ API parity test (every v6-catalog capability exists in the API
  and vice versa); `pi-ext` 0.x section in [stability-contract.md](../../stability-contract.md).
  *Acceptance:* parity test green; stability contract updated.
- **T5 — `pi-ext`: user target (v6 process).** Runtime glue that turns an
  extension's `main` into a v6-speaking process: stdio framing, `init`/
  `ready` handshake, dispatch loop (F&F + decision), UI request round-trip,
  clean error response on panic. Example extension + NDJSON test harness.
  *Acceptance:* the example extension is driven by a harness (same pattern as
  `tests/extension_bridge.rs`): ready catalog, tool round-trip, decision
  block, UI dialog.
- **T6 — `pi-ext`: core target (C ABI + dlopen).** `pi_ext_core_init_v1` +
  `PiCoreApi` vtable (JSON payloads, `user_data`, `PI_EXT_CORE_API_VERSION =
  1`), `catch_unwind` boundary, example cdylib
  ([ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md)).
  *Acceptance:* a test host `dlopen`s the example `.so`, registers a tool +
  handler, invokes both; a panicking example is caught, marked faulted, host
  continues.

### Phase 2 — core runtime (E1S3)

- **T7 — N-peer runtime.** Generalize `BridgeClient` (one 1:1 child) to a
  per-peer state machine + peer set: spawn N Rust extension peers in
  parallel, `ready` aggregation, F&F fan-out, decision combination in
  architecture order (core → sidecar → user peers path-sorted; first `block`
  wins), tool merge + collision diagnostic, per-peer `Degraded` (its tools
  dropped, others continue, no respawn) ([ADR 0001](../../adr/0001-rust-extensions-as-bridge-subprocesses.md)).
  *Acceptance:* integration test with two fake v6 Rust-peer fixtures + the
  sidecar in one session; killing one peer mid-session drops only its tools
  and keeps exit semantics.
- **T8 — Core-owned discovery + protocol v7 + trust.** Core scans the three
  locations and classifies: `pi-ext.toml` dir → Rust peer; TS shapes → sidecar
  list. Bridge **v7** = v6 + `init.extensions: [{path, kind}]`; the sidecar
  loads exactly the given list (no own discovery). Project-scope Rust
  extensions inert until trusted (WP-1102 pattern, [ADR 0004](../../adr/0004-rust-extensions-share-discovery-and-trust.md)).
  *Acceptance:* trust e2e (untrusted project Rust extension: no spawn, zero
  effect; grant trust → loads), sidecar e2e with the explicit list, v7
  handshake tests.
- **T9 — Port `hello` (first core extension).** `core-ext/hello` cdylib on
  the pi-ext core target; same tool name/schema, same output
  ([ADR 0005](../../adr/0005-bridge-matrix-ports-as-core-extensions.md)).
  *Acceptance:* the same e2e scenario as the unmodified original
  (`extension_tool_call` → `Hello, <name>!`) — via the T6 test host (in-core
  e2e arrives with T10).
- **T10 — Core-extension loading in the core.** `dlopen` `core-ext/*.so` at
  startup (after trust resolution, name order): manifest sha256 verification,
  `PI_EXT_CORE_API_VERSION` check, in-process registry, dispatch in the agent
  loop (decision order per architecture.md), faulted library → skip + log,
  `PI_RUST_CORE_EXT_DIR` ([ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md)).
  *Acceptance:* a Rust-only run spawns no Node process (test asserts no
  sidecar spawn); `hello` e2e green in the real core; a faulted example
  library → session continues, other libraries work; sha256 mismatch refused
  + logged.
- **T11 — Mixed session + bench re-check.** One session runs a TS extension
  (sidecar) + a Rust user extension (subprocess) + a core extension
  simultaneously; the `--bench` loop re-pointed at a Rust peer.
  *Acceptance:* mixed e2e green (all three sources' tools callable, decision
  order observable); measured `bridge_events_per_sec` against a Rust peer
  recorded in [benches/baseline.json](https://github.com/ripmav/pi-rust/blob/main/benches/baseline.json).

### Phase 3 — remaining ports (E1S4)

Each: same e2e scenario as the unmodified original (same fixtures), in-core.

- **T12 — Port `permission-gate`.** Regex block + `ui.select`; headless
  block-by-default. *Acceptance:* dangerous bash blocked with reason
  (headless) / confirmed via dialog (TUI) — as the original.
- **T13 — Port `claude-rules`.** Filesystem walk + `session_start` notify +
  `before_agent_start` injection. *Acceptance:* `.claude/rules/` files listed
  in the system prompt — as the original.
- **T14 — Port `prompt-customizer`.** `systemPromptOptions.sections.
  tool_guidance` mutation. *Acceptance:* guidance lines present per active
  tools/skills — as the original.
- **T15 — Port `with-deps`.** Own external dependency (Cargo crate for
  duration parsing, e.g. `humantime`). *Acceptance:* `1h` → `3600000` ms —
  as the original.
- **T16 — Port `plan-mode` + porting guide.** The deep port: flag, commands,
  shortcut, tool-set management, `[DONE:n]` tracking, persistence/resume,
  status/widget/editor/select. Plus the porting guide in `docs/` (per-
  extension mapping table upstream API → `pi-ext`, hard parts annotated).
  *Acceptance:* `--command plan` toggle, bash allowlist block, `[PLAN MODE
  ACTIVE]` injection, `appendEntry` persistence + resume, TUI widget/status
  — as the original; guide exists.

### Phase 4 — authoring framework (E1S5)

- **T17 — Scaffolding + authoring docs.** `pi-rust ext new <name>` (or cargo
  template): working skeleton (tool + command + event handler) + `pi-ext`
  dependency + test-harness wiring; authoring docs: discovery paths
  (`pi-ext.toml`), trust, UI, versioning, running an extension standalone
  ([ADR 0003](../../adr/0003-workspace-with-pi-ext-crate.md)).
  *Acceptance:* the scaffolded extension compiles and passes the extension
  test harness out of the box; docs cover discovery/trust/UI/versioning.

## Log

- **L1** (2026-11-21): Epic E1 + ADRs 0001–0006 + `docs/architecture.md`
  recorded (PR #150); E1S1 done. This task file written; next: T1.
