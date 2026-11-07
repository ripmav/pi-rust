# pi-rust

Rust port of [pi](https://github.com/earendil-works/pi) with a compatibility
layer: original pi extensions (TypeScript, unmodified) run unchanged against
the new Rust core.

**Status: PoC — "bridge first".** The extension compatibility layer is
validated end-to-end; the Rust core is a minimal scripted agent behind it
(fake model driver, scenario runner, no TUI, no real providers). See
`odd/tasks/extension-bridge.md` for the full spec/task log.

## Architecture

```
┌────────────────────────────┐   NDJSON over stdio    ┌───────────────────────────────────┐
│  pi-rust core (Rust)       │ ─────────────────────► │  sidecar (Node)                   │
│  - agent loop, model driver│ ◄───────────────────── │  - ORIGINAL upstream extension    │
│  - built-in tools: bash,   │  commands / responses  │    loader + ExtensionRunner,      │
│    read, write             │  / events / dialogs    │    unmodified extensions          │
│  - session (JSONL v0)      │                        │  - ExtensionAPI (headless slice)  │
└────────────────────────────┘                        └───────────────────────────────────┘
```

- **Core (Rust)** owns the agent loop, the (fake) model, built-in tools, and
  session persistence. It is the RPC client.
- **Sidecar (Node)** runs the *original, unmodified* upstream extension loader
  (`packages/coding-agent` built `dist/`) and `ExtensionRunner`. It is the RPC
  server. It never talks to a model and never writes session files.
- Protocol: **bridge protocol v0** — NDJSON over stdio, framing and event
  shapes borrowed from upstream's own headless RPC mode. Full spec:
  [`docs/bridge-protocol.md`](docs/bridge-protocol.md).
- Upstream pin: `earendil-works/pi @ 085c84e` (shallow clone in `upstream/`,
  gitignored). The sidecar imports the built `dist/`, so **upstream must be
  built** before running (below). Bumping the pin may require re-checking the
  import surface (`discoverAndLoadExtensions`, `createEventBus`,
  `ExtensionRunner`, `wrapRegisteredTools`, `buildSystemPrompt`,
  `normalizeBuildSystemPromptOptions`).

## Compatibility scope (v0)

Unmodified upstream example extensions run "wie gewohnt" for the
**headless-safe API slice**: `on` (all catalog events), `registerTool`
(TypeBox schemas, nested `ctx.executeTool`), `registerCommand` (invoked via
`--command`), `registerFlag`/`getFlag`, `sendMessage`/`sendUserMessage`,
`appendEntry` (persisted by the core), session name, `exec`, `getSettings`,
`setModel`/thinking level, `registerProvider` (reported, not invoked),
`registerMcpServer` (reported only), `pi.events`, dialog subset
(`select`/`confirm`/`input`/`editor`/`notify`/`setStatus`/`setTitle`/
`set_editor_text` — answered with upstream defaults in v0, no human UI).

Out of scope for v0: TUI renderers/components, `registerShortcut`, themes,
compaction events, MCP transport, provider invocation, `ctx.ui.custom`,
interactive REPL/TUI. Details in
[`docs/bridge-protocol.md` §Out of scope](docs/bridge-protocol.md).

Validated end-to-end (unmodified upstream examples, `tests/extension_bridge.rs`):
`hello`, `with-deps` (npm deps), `permission-gate`, `claude-rules`,
`plan-mode`, `prompt-customizer` + the degraded sidecar-crash path.

## Quickstart

Prerequisites: Node ≥ 26 (or whatever the upstream build targets), Rust
stable, npm.

```sh
# 1. Build upstream once (the sidecar runs its dist/):
cd upstream && npm ci --no-audit --no-fund && npm run build && cd ..

# 2. Run the test suite (spawns real sidecar + unmodified extensions):
cargo test                 # 6 e2e integration tests
node sidecar/smoke.mjs     # 34 sidecar protocol assertions (fake core)

# 3. Run a scenario:
source $HOME/.cargo/env    # if rustup is not on PATH
cargo run -- --extension-dir upstream/packages/coding-agent/examples/extensions/hello.ts \
  --scenario scenario.json
```

### CLI

```
pi-rust --extension-dir <path> (repeatable)
        [--agent-dir <path>]          # isolated discovery dir (default: temp)
        --scenario <scenario.json>    # required
        [--session-file <path>]       # default: .pi-rust/sessions/<uuid>.jsonl
        [--command <name> [args...]]  # repeatable; run after ready, before the scenario
```

Env: `PI_RUST_SIDECAR` (sidecar entry override), `PI_RUST_NODE` (node binary).
The sidecar is found by walking up from the cwd, then from the executable's
directory.

### Scenario format

```json
{
  "user_messages": ["first user message", "second user message"],
  "model_scripts": [
    [
      { "tool_call": { "name": "bash", "input": { "command": "echo hi" } } },
      { "tool_call": { "name": "hello", "input": { "name": "bridge" } } },
      { "text": "final assistant text" }
    ],
    [ { "text": "second message reply" } ]
  ]
}
```

`model_scripts[i]` is the scripted output of the fake model for
`user_messages[i]` (consecutive `tool_call` entries form one multi-call turn).
Unknown tool names abort the run (exit 2) — that is a test assertion signal.

### Output contract

- **stdout**: machine-readable NDJSON event log — `user_input`, `decision`
  (request + response per decision event), `tool_execution` (final input,
  `blocked`, result summary), `model_output`, `command`, `extension_error`,
  `extension_ui_request`, turn boundaries, and a final `transcript` line with
  the full session.
- **stderr**: human progress + sidecar logs.
- **Exit codes**: `0` clean · `1` fatal (startup/config) · `2` model-script
  or shutdown failure · `3` degraded (sidecar crashed mid-run; built-ins kept
  working, no respawn — protocol: degraded is terminal).

The session file is pi-rust's own minimal v0 JSONL format
(user/assistant/tool_result + extension custom entries) — **not** upstream
session-file compatible (a later milestone).

## Repository layout

```
src/            Rust core (transport, client, model, tools, session, agent, cli)
sidecar/        Node sidecar (protocol server, API shim, dialogs, session view, smoke)
tests/          e2e integration tests (spawn core + sidecar + unmodified extensions)
tests/fixtures/ test-only fixture extensions (clearly marked; not upstream code)
docs/           bridge-protocol.md — the protocol contract
odd/tasks/      feature spec + task log (S1–S5, L1–L11)
upstream/       earendil-works/pi @ 085c84e (gitignored reference clone)
```

## Next milestones (not in this PoC)

Interactive TUI (ratatui) with theme/keybinding parity, real model drivers
(OpenAI-compatible first), compaction, MCP transport, upstream session-file
compat, provider invocation of `registerProvider` extensions, sidecar respawn.
