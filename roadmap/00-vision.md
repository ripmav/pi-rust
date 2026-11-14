# Vision, architecture, compatibility contract

## Goal

A Rust implementation of pi that:

1. **Matches pi's behavior** for the parts pi users actually run: interactive
   agent, model providers, built-in tools, sessions, config, extensions.
2. **Runs existing extensions unmodified** — the headline promise. Original
   TypeScript extensions (the entire `packages/coding-agent` extension API
   surface) load and behave as in upstream pi.
3. **Keeps the Rust core fast and auditable**: one binary, minimal deps,
   no Node runtime in the core.

## Architecture (proven in M1/M2)

```
┌────────────────────────────┐        NDJSON over stdio (bridge protocol v0)
│  Rust core (pi-rust)       │◄────────────────────────────►┐
│  agent loop, session,      │                              │
│  model providers (native), │                              │
│  built-in tools, (later)   │                              │
│  TUI                       │                              │
└────────────────────────────┘                              │
                                                            ▼
                                            ┌────────────────────────────────┐
                                            │ Node sidecar                  │
                                            │ ORIGINAL upstream loader +    │
                                            │ ExtensionRunner (jiti, npm    │
                                            │ deps, events) — unmodified    │
                                            │ extension code runs here      │
                                            └────────────────────────────────┘
```

- **Why a sidecar?** Upstream extensions are ESM TypeScript loaded through
  `jiti` with arbitrary npm deps — a QuickJS/snapshot port is not viable in
  v1. The sidecar reuses upstream's own `discoverAndLoadExtensions` +
  `ExtensionRunner`, so loader behavior is identical by construction.
- **The core owns** model I/O, session state, tool dispatch, and (later) the
  TUI. The sidecar never talks to a model.
- **Bridge protocol v0** (`docs/bridge-protocol.md`): NDJSON, upstream RPC
  framing. Stable since M1; provider selection is core-side and added no wire
  messages (M2). Protocol changes require a spec version bump + sidecar/Core
  lockstep.

## Compatibility contract

Upstream pi extensions run unmodified for this surface (status per milestone):

| Extension API | Status |
|---|---|
| `on(event)` for all catalog events | ✅ M1 |
| `registerTool` (TypeBox schemas, nested `ctx.executeTool`) | ✅ M1 |
| `registerCommand`, `registerFlag`/`getFlag` | ✅ M1 |
| `sendMessage`/`sendUserMessage`, `appendEntry` | ✅ M1 |
| session name, `exec`, `getSettings` | ✅ M1 |
| `setModel` (reported), thinking level (gated on core ack) | ✅ M1/M2 |
| dialog *handling* (answered with upstream defaults) | ✅ M1 (headless) |
| real human dialog answers (select/confirm/input/editor) | M6 (TUI) |
| model providers: openai-compat + anthropic | ✅ M2 |
| remaining built-in providers (google, bedrock, azure, cloudflare, openrouter, ollama, …) | M4 |
| runtime `setModel` switching | M4 |
| upstream session-file format, `--resume` | M3 |
| `~/.pi/agent` config, model catalog, auth storage | M3 |
| built-in tools parity (read/write/edit/bash) | M5 |
| `registerProvider` (custom providers invoked) | M7 |
| `registerMcpServer` (real MCP transport) | M7/M8 |
| compaction events + auto-compaction | M7 |
| `registerShortcut`, themes | M6/M7 |
| `ctx.ui.custom` | evaluate (WP-707); possibly non-goal |

## Principles

1. **Upstream is the reference**, pinned per milestone (currently `17d6ca7`);
   behavior questions are answered by reading upstream source.
2. **Small work packages**: one issue, one reviewable PR, TDD where runnable
   checks exist (RED → GREEN), mock servers for provider tests (no live APIs
   in CI).
3. **Extension compatibility is a test claim**: each milestone that touches
   the API surface re-runs the unmodified-extension matrix.
4. **Native review (RDD)** for high-risk units; verify-always otherwise.
5. **One PR per delivery slice**; work-unit commits inside.

## Non-goals (v1)

- Upstream internal/experimental packages: `chord`, `codemode`, `durable`,
  `evals`, `telemetry` (revisit per package with justification).
- Pixel-perfect TUI parity with the Ink/React implementation — *functional*
  parity of interactive features (ratatui).
- Multi-agent orchestration beyond pi's own model.
- Windows support until CI proves out macOS/Linux (upstream targets macOS/
  Linux first).
