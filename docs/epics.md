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
- **M14 — core-native model providers** → E3

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
| **E2S1** Custom providers from `models.json` (OpenAI-compatible) | A user adds a provider to `~/.pi/agent/models.json` (`api: "openai-completions"`, `baseUrl`, `apiKey` with `$ENV_VAR`) and uses it with pi-rust — no extension shim required | The sidecar's upstream `ModelRuntime` already loads `<agentDir>/models.json` (upstream `model-runtime.ts` `providerIds()` includes the config provider ids); WP-1301 (delivered sidecar-side as the interim — superseded by E3: the core owns models.json since E3S1, the sidecar's branches were removed in E3S2, and the E2S1 e2e is re-pointed at the core-native path): the sidecar advertised models.json providers in `ready.providers` and routed their `provider_stream` through the upstream runtime (credential resolution is upstream's, incl. `$ENV_VAR` interpolation) — no core changes; the user's solheim case works end-to-end without a shim (e2e: models.json provider against a local mock OpenAI SSE endpoint, transcript carries the mock's response); missing `models.json` is a no-op, a malformed one does not start the run differently than before WP-1301 (sidecar runtime creation unchanged); models.json providers are selectable via `--provider`/`--model` and via `settings.json` `defaultProvider`/`defaultModel`; documented v0 limit: when models.json **overlays a core built-in** provider id, the core's bundled catalog wins for that id | done |

---

## Milestone 14 — core-native model providers

Implementation tasks: [odd/tasks/m14-core-native-model-providers.md](odd/tasks/m14-core-native-model-providers.md)

### Epic E3 — Core-native model providers (sidecar = js/ts extensions only)

**Why:** the sidecar's purpose is to load and run the original js/ts extensions (the README's "compatibility layer"). WP-1301 (E2S1) put models.json / model-runtime work there as a stopgap, because the sidecar's upstream `ModelRuntime` already had it — but the design principle (user, 2026-11-23) is that the sidecar is **only** for js/ts extensions. The core must own model providers natively: parse `<agentDir>/models.json` in Rust and stream OpenAI-compatible providers through its built-in drivers, so provider support does not depend on the sidecar. Together with E1S3 (native Rust extension runtime), this is the path to a truly Node-free run. **End state (E3S5, 2026-11-24): the core owns models/providers + MCP (+ the oauth builtin it can natively stream)** — the sidecar hosts no MCP servers (D13, PO 2026-11-24) and no model providers; its remaining Node duties are the js/ts extensions, the remaining js/ts built-in extension hosting (tool-search/llama/codemode, until their own core-native stories, if the user wants them), and — until **E4S2** (M15 provider plugin platform) is green — the four wire-API-less oauth builtins (radius, github-copilot, xai, meta; D15 answered 2026-11-24: the core does all API interactions incl. logins, as auto-loaded provider libraries).

> Direction (user/PO, 2026-11-23): "the sidecar only is to load and execute the js/ts extension. So we have to create an epic to change this!" — answered [D8](decisions-open.md) (core-native, tracked here); the E2S1 sidecar path was the interim — E3S2 removed it.

| Story | I want | Acceptance | Status |
|---|---|---|---|
| **E3S1** Core-native `models.json` + OpenAI-compatible streaming | The core loads models.json itself and streams OpenAI-compatible providers without the sidecar | The Rust core parses `<agentDir>/models.json` (upstream schema subset: `providers{name, baseUrl, api, apiKey, models[{id, name, reasoning, input, cost, contextWindow, maxTokens}]}`; JSONC like upstream: BOM + `//`/`/* */` comments; missing file = no-op; malformed = typed error, run continues with the bundled catalog); parsed providers merge into the core's model registry — `--provider`/`--model`, `settings.json` `defaultProvider`/`defaultModel` and model listing behave exactly as on the E2S1 sidecar path (same selection e2e, provider call over the core driver); `api: "openai-completions"` streams through the core's built-in OpenAI driver with the provider's `baseUrl` + resolved `apiKey` (literal + `$ENV_VAR`; auth.json via the existing core credential store); e2e: the solheim case (mock OpenAI SSE endpoint) works with the **sidecar degraded** (spawn failure) — the provider call never crosses the bridge; other api types (e.g. `anthropic-messages`) on models.json providers: typed "not core-native yet" error, no crash | done (WP-1401) |
| **E3S2** The sidecar is js/ts-extensions-only again (providers) | After E3S1, models.json stops living in the sidecar | The WP-1301 models.json advertise + stream branches in the sidecar are removed (incl. their smoke phase + the E2S1 e2e is re-pointed at the core-native path); the sidecar's documented role is "js/ts extensions" (+ its built-in extension hosting — mcp/tool-search/llama/codemode, until E3S3/E3S4/E3S5); README, `docs/bridge-protocol.md` (ready providers sources) and E2S1's status note updated (delivered sidecar-side as interim, superseded by E3); the user's solheim scenario still works e2e with no shim via the core-native path | done (WP-1402) |
| **E3S3** Core-native MCP hosting ("then the rest" — D9) | MCP servers are connected and driven by the core, not the sidecar | The Rust core speaks MCP itself (jsonrpc over stdio + streamable HTTP) for the same server sources as today (settings `mcpServers` + project `.pi/` under the trust gate); MCP tools appear in the model's tool set with upstream naming (`mcp__<server>__<tool>`) and upstream schemas; tool calls execute natively (no `tool_call_request` hop for MCP tools); the `mcp_servers_change`/connect-on-session_start semantics are preserved (same observable events); e2e: the existing MCP fixture scenarios (stdio + HTTP, `exposure` rules) pass with the **sidecar degraded** | done (WP-1403; D13 answered by the PO 2026-11-24 — core owns all MCP, sidecar hosts none; E3S5 then removed the sidecar mcp path entirely — the `oauth`/`auth`-configured servers are skipped by the core with a warning and have no host now, D17) |
| **E3S4** Core-native oauth builtins ("then the rest" — D9) | The five upstream oauth providers (radius, kimi-coding, github-copilot, xai, meta) run through the core | `auth login` for those providers, credential storage in the core's auth store (auth.json, same format), token refresh and rotated write-back happen in the Rust core (no sidecar hop, WP-1007's "no token crosses the wire" property now trivially holds — nothing leaves the process); the oauth builtins appear in the core's model registry and stream through the core's drivers with the stored credential; e2e: an oauth-builtin provider run works with the **sidecar degraded** (mock flows for refresh/rotation) | done (WP-1404; the core owns `kimi-coding` end-to-end — account login (device flow), auth.json storage, refresh + rotated write-back, and streaming through the built-in driver, e2e against a local mocked oauth server with the sidecar degraded; the other four builtins (radius, github-copilot, xai, meta) have no core wire-API driver yet — they stay sidecar-side until E4S2 (M15) is green: D15 answered 2026-11-24, the core does all API interactions incl. logins as auto-loaded provider libraries) |
| **E3S5** The sidecar is js/ts-extensions-only (final form) | After E3S3/E3S4, nothing but js/ts extensions runs in the sidecar | The sidecar's mcp + oauth branches are removed (built-in mcp extension hosting for servers, `OAUTH_BUILTIN_PROVIDERS` routing for what the core now owns — incl. its smoke phases + e2e re-pointed at the core paths); the four oauth builtins the core cannot stream yet (radius, github-copilot, xai, meta) **stay sidecar-side until E4S2 is green** (D15 answered 2026-11-24 — the core does all API interactions incl. logins, as auto-loaded provider libraries; no capability regression mid-epic); the sidecar's documented role is js/ts extensions (+ the remaining built-in extension hosting that is itself js/ts — tool-search/llama/codemode, or their own core-native stories if the user wants them); README, `docs/bridge-protocol.md`, [architecture.md](architecture.md) and E3's "Why" updated; **full regression: a sidecar-degraded run works for providers + MCP + oauth builtins** (the E3 end-state test); **PO decision walkthrough 2026-11-24 (implemented in the same PR, task log L16):** the sidecar's `ModelRuntime` is created with `modelsPath: null` — **no direct models.json access at all** (D11 — pinned by a sidecar check that the sidecar starts normally with a present/malformed models.json in the agent dir, and that a models.json overlaying a builtin does not alter the advertised builtin catalog), and `pi.registerMcpServer` calls from extensions are **forwarded to the core** (the core owns the connection — the sidecar hosts no MCP servers at all; the core is the single source of truth for MCP, D13 — new additive bridge message `mcp_server_register` in v6; a forward failure is a typed extension-side error; the core's `mcp_servers_change` forward now carries the full set, configured + extension-registered) | done (WP-1405; removed from `sidecar/bridge.mjs`: `loadBuiltinMcpExtension` + its init push and the `mcpConfig` registration path — the core is the single source of truth for MCP (D13, PO 2026-11-24) and no longer sends `mcpConfig`; `pi.registerMcpServer` is accepted by the unmodified runner but never connected (no host remains; forwarding the registration to the core is the follow-up, task log L15). The four oauth builtins stay sidecar-side (per acceptance above — E4S2 unblocks their removal; the sidecar's oauth branch for what the core owns was already removed in E3S4: kimi-coding). Deletions pinned by negative checks in `sidecar/smoke.mjs` (no `builtin:mcp`; an init WITH `mcpConfig` is ignored — the fixture server is never spawned, no `mcp__` tools; 10 extensions) and the e2e `wp1405_register_mcp_server_not_hosted_pin`; the e2e `wp1405_sidecar_degraded_full_regression` is the end-state test — one test, three sub-runs with the sidecar spawn failing: a models.json model streams, an MCP tool is listed/callable with its result in the transcript, and the kimi-coding oauth builtin (mocked token, core refresh) streams. `oauth`/`auth`-configured MCP servers have no host (D17). Smoke baseline 76/76 (the (m) phase re-pinned negative) — **follow-up (task log L16) shipped in the same PR:** `modelsPath: null` (D11) + the `registerMcpServer` forward (D13) — smoke re-baselined to **90/90** (new phases (o) D11 present/malformed/overlay + (p) D13 forward/rejection), `wp1405_register_mcp_server_not_hosted_pin` re-pinned: the sidecar still hosts nothing (ready: 5 extensions, no `builtin:mcp`) while the server now shows up core-side (declared, callable natively, spawned exactly once, in the `mcp_servers_change` full set) |

**Sequencing (D9 answered 2026-11-23):** first models.json + OpenAI-compatible providers (E3S1 → E3S2), **then the rest** — E3S3 (MCP) → E3S4 (oauth) → E3S5 (final sidecar cleanup). Each story's sidecar-removal half ships only after the corresponding core path is green, so the user-facing capability never regresses mid-epic. E3S5 closed the sequence (2026-11-24): the core owns models/providers + MCP (+ the oauth builtin it can natively stream); the sidecar hosts js/ts extensions — and, until E4S2 (M15) is green, the four wire-API-less oauth builtins. The final js/ts-extensions-only form ships with E4S2.

---

## Milestone 15 — Provider Plugin Platform (core-native API libraries)

> Direction (user/PO, 2026-11-24, answering [D15](decisions-open.md)): "API interaktionen, auch logins soll der core machen. APIs sollen aber als library vorliegen die automatisch geladen werden (aus einem entsprechenden verzeichnis)." — the core does **all** provider API interactions (streaming and oauth logins); provider implementations are libraries the core loads automatically from a directory at startup. Plugin format (D16, answered by the PO 2026-11-24, option 1): **Rust cdylibs with a stable C ABI vtable + JSON payloads** (the ADR 0005/0006 extension-runtime pattern), auto-loaded from the bundled dir shipped with the binary + `<agentDir>/providers/` (user dir); the compiled-in builtins stay compiled-in for the MVP.

| Story | What | Acceptance criteria | Status |
|---|---|---|---|
| **E4S1** Provider plugin ABI + directory loader | The core loads provider libraries (plugins) from a directory at startup | A stable plugin ABI (stream, auth/login handshake, model listing) as **Rust cdylibs with a C ABI vtable + JSON payloads** (ADR 0005/0006 pattern; D16 answered 2026-11-24: option 1); auto-loaded at startup from the **bundled dir shipped with the binary** + the **`<agentDir>/providers/`** user dir; a failing plugin is a typed log line (the run continues with the remaining providers); plugins never touch the credential store directly — auth.json storage and the login UX stay core-side; TDD: sample plugin (listed, streamable, login flow against a mock) + no sidecar involved | open |
| **E4S2** The four remaining oauth builtins as core plugins | radius, github-copilot, xai, meta run through the core (login + streaming) | each of the four is a plugin implementing its wire API (`pi-messages` + dynamic catalog for radius; multi-API + account proxy + `availableModelIds` for github-copilot; `openai-responses` for xai/meta); login flows (device/browser/loopback) core-driven, tokens in auth.json (upstream shape), refresh + rotated write-back core-side; e2e (mocked flows): each provider selectable + streamable + login-able with the **sidecar degraded**; unblocks E3S5's sidecar oauth removal (sequencing rule) | open |
| **E4S3** Compiled-in builtins as bundled plugins (uniformity — D16 option 2) | openai/anthropic/google/bedrock also live as plugins in the bundled dir | the built-in drivers are delivered through the same plugin mechanism — one code path for all providers; full suite + bench baseline unchanged. **Backlog (PO, 2026-11-24):** to be implemented later — no value for the MVP; D16 option 1 (builtins stay compiled-in) ships first | backlog |

**Sequencing (D15/D16 answered 2026-11-24):** E4S1 → E4S2; E4S3 sits in the backlog (PO: later, no MVP value). E3S5's sidecar oauth removal for the four builtins ships only after E4S2 is green (E3 sequencing rule — no capability regression mid-epic); until then the four stay sidecar-side (the D15 interim).

Implementation tasks: [odd/tasks/m15-provider-plugin-platform.md](odd/tasks/m15-provider-plugin-platform.md)

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
