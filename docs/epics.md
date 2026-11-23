# Epics & Stories — pi-rust

pi-rust is a Rust port of earendil-works/pi: a native agent core (model runtime,
tools, session storage) with a TUI, plus a [Node sidecar](https://github.com/ripmav/pi-rust/tree/main/sidecar)
that runs the original, unmodified TypeScript extensions over the NDJSON
[bridge protocol](bridge-protocol.md) ([stability contract](stability-contract.md)).
From M12 on, the plan lives in this file; M1–M11 history stays in
[roadmap/01-milestones.md](roadmap/01-milestones.md) and
[odd/tasks/](odd/tasks/).

---

## Milestones

- **M11 — v1: project trust & sealing** → delivered 2026-11-21 (v1.0.0 released); history in the milestone/feature docs linked above
- **M12 — native Rust extension platform** → E1
- **M13 — custom model providers** → E2

---

## Milestone 12 — native Rust extension platform

Implementation tasks: [odd/tasks/m12-rust-extension-platform.md](odd/tasks/m12-rust-extension-platform.md)

### Epic E1 — Native Rust extension interface & framework

**Why:** Every extension today is TypeScript running in the Node sidecar — each
extension run needs a Node runtime, and the authoring language is fixed to
JS/TS. A native interface lets extensions be written in pure Rust (a Rust-only
install needs no Node at all), lets existing extensions be ported to Rust, and a
framework (public API crate + scaffolding + docs) makes creating new extensions
easy. The sidecar is **not** replaced: the Rust interface runs beside it, so TS
and Rust extensions coexist.

> Requirement (user, 2026-11-21): "Beside the current the sidecare to run
> extension of the origin pi, want to implement a extension interface so that
> new extensions can be created in rust or old ones ported to rust. Also there
> should be a framework for more easy creating extensions."

| Story | I want | Acceptance | Status |
|---|---|---|---|
| **E1S1** Extension model design (ADRs) | A recorded decision on how Rust extensions are loaded, addressed, and wired into the core, before any code | Loading model, API surface, crate layout and discovery/trust recorded in [ADR 0001](adr/0001-rust-extensions-as-bridge-subprocesses.md), [ADR 0002](adr/0002-mirror-full-upstream-extension-api.md), [ADR 0003](adr/0003-workspace-with-pi-ext-crate.md) and [ADR 0004](adr/0004-rust-extensions-share-discovery-and-trust.md); porting set recorded in [ADR 0005](adr/0005-bridge-matrix-ports-as-core-extensions.md); a `docs/architecture.md` created, describing the extension runtime (core ↔ native extensions ↔ sidecar, bridge protocol, core-extension install location); story acceptance criteria E1S2–E1S5 refined to cite the ADRs; runtime topology, C ABI, discovery/classification, event flow and install layout specified in [architecture.md](architecture.md) | done |
| **E1S2** Extension framework crate (public API) | A public crate extension authors code against, mirroring the upstream extension API (`pi` object + `ctx`), with stable types | The `pi-ext` crate ([ADR 0003](adr/0003-workspace-with-pi-ext-crate.md)) builds and tests standalone and supports both extension targets — the v6-speaking binary (user extensions) and the cdylib (core extensions, [ADR 0006](adr/0006-core-extensions-load-in-process-as-so.md)); defines the v1 surface per [ADR 0002](adr/0002-mirror-full-upstream-extension-api.md) (full 1:1 mirror, catalog ↔ API parity test); doc examples compile (`cargo test --doc`); the [stability contract](stability-contract.md) gains a section covering the extension API surface | open |
| **E1S3** Native extension runtime in the core | The core loads and runs Rust extensions directly, without Node | An example Rust extension registers a tool, a command and event handlers; e2e green headless and TUI (tool-call round-trip, command invocation, `before_agent_start` system-prompt injection, UI dialog round-trip with a UI attached); a Rust-only run spawns no Node process (test asserts no sidecar spawn); mixed session works — one session runs a TS extension (sidecar) and a Rust extension (native) simultaneously; project-scope Rust extensions are gated by the trust model (WP-1102, [ADR 0004](adr/0004-rust-extensions-share-discovery-and-trust.md)); a **user**-extension crash never kills the core (subprocess semantics per [ADR 0001](adr/0001-rust-extensions-as-bridge-subprocesses.md)); **core** extensions ([ADR 0005](adr/0005-bridge-matrix-ports-as-core-extensions.md), [ADR 0006](adr/0006-core-extensions-load-in-process-as-so.md)) load in-process as packed `.so` files by default; the `--bench` loop re-run against a Rust extension peer records a measured `bridge_events_per_sec` in [benches/baseline.json](https://github.com/ripmav/pi-rust/blob/main/benches/baseline.json) | open |
| **E1S4** Ported core extensions | The six bridge-matrix TS extensions ported to Rust as first-party core extensions, with a porting guide | The six (`hello`, `permission-gate`, `claude-rules`, `prompt-customizer`, `with-deps`, `plan-mode`) are ported to Rust and pass the same e2e scenarios as the unmodified originals (same fixtures); they ship as core extensions — packed into the release, loaded in-process as `.so` files by default, core scope (trusted by definition, [ADR 0005](adr/0005-bridge-matrix-ports-as-core-extensions.md) + [ADR 0006](adr/0006-core-extensions-load-in-process-as-so.md)); `with-deps` carries its own external dependency (npm `ms` → Cargo crate); a porting guide in `docs/` maps upstream API → framework API per extension | open |
| **E1S5** Authoring framework (scaffolding + docs) | Creating a new extension takes minutes, not an afternoon | Scaffolding per [ADR 0003](adr/0003-workspace-with-pi-ext-crate.md) (e.g. `pi-rust ext new <name>` or a cargo template) produces a working skeleton (tool + command + event handler); the scaffolded extension compiles and passes the extension test harness out of the box; authoring docs cover discovery paths ([ADR 0004](adr/0004-rust-extensions-share-discovery-and-trust.md)), trust, UI and versioning | open |

---

## Milestone 13 — custom model providers

Implementation tasks: [odd/tasks/m13-custom-model-providers.md](odd/tasks/m13-custom-model-providers.md)

### Epic E2 — Custom providers with OpenAI-compatible endpoints

**Why:** pi-rust only knew the bundled upstream catalog plus providers registered by extensions; a user-configured provider (upstream `models.json` format — e.g. an OpenAI-compatible endpoint) failed at startup with `fatal: provider X is not a built-in provider and was not registered by an extension`, forcing a hand-written extension shim per provider. Upstream pi reads `<agentDir>/models.json` natively; pi-rust mirrors that. (M8's protocol-diff acceptance test already pinned `models.json` for the upstream half of the comparison.)

> Requirement (user/PO, 2026-11-23): "pi-rust MUST support custom providers with OpenAPI-compatible endpoints" — originally typed "OpenAPI-compatible"; confirmed 2026-11-23 that **OpenAI-compatible** was intended (D7 answered; a user type).

| Story | I want | Acceptance | Status |
|---|---|---|---|
| **E2S1** Custom providers from `models.json` (OpenAI-compatible) | A user adds a provider to `~/.pi/agent/models.json` (`api: "openai-completions"`, `baseUrl`, `apiKey` with `$ENV_VAR`) and uses it with pi-rust — no extension shim required | The sidecar's upstream `ModelRuntime` already loads `<agentDir>/models.json` (upstream `model-runtime.ts` `providerIds()` includes the config provider ids); WP-1301: the sidecar advertises models.json providers in `ready.providers` and routes their `provider_stream` through the upstream runtime (credential resolution is upstream's, incl. `$ENV_VAR` interpolation) — no core changes; the user's solheim case works end-to-end without a shim (e2e: models.json provider against a local mock OpenAI SSE endpoint, transcript carries the mock's response); missing `models.json` is a no-op, a malformed one does not start the run differently than before WP-1301 (sidecar runtime creation unchanged); models.json providers are selectable via `--provider`/`--model` and via `settings.json` `defaultProvider`/`defaultModel`; documented v0 limit: when models.json **overlays a core built-in** provider id, the core's bundled catalog wins for that id | done |

---

## Cross-cutting

- **Trust gate (WP-1102) applies to Rust extensions:** untrusted project-scope
  content (`<cwd>/.pi/`) has zero behavioral effect; per-path trust unchanged.
  Core extensions (in-process, [ADR 0006](adr/0006-core-extensions-load-in-process-as-so.md)) are
  first-party and trusted by definition — the same trust level as built-in
  tools.
- **Upstream pin** `earendil-works/pi @ 64a15f4` defines the extension API the
  framework mirrors; pin-cycle refreshes (M11 pattern) re-verify the mirror.
- **No regression of the TS path:** the sidecar and bridge protocol (v6) remain
  the TypeScript path; the existing extension e2e matrix stays green.

## Out of scope (v1)

- Replacing or removing the Node sidecar or the TS extension path.
- Auto-transpiling TypeScript extensions to Rust.
- Publishing the framework crate to crates.io (workspace/local distribution
  first; revisit after E1S3).

## Open questions

See [decisions-open.md](decisions-open.md).
