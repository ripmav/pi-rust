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
│  - session (upstream v3)   │                        │  - ExtensionAPI (headless slice)  │
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

### Agent dir, `~/.pi` and extension discovery

The core's **agent dir** resolves like upstream `getAgentDir`:
`--agent-dir` > env `PI_CODING_AGENT_DIR` > `~/.pi/agent`. **By default the
core reads your real `~/.pi/agent`** (M3): `settings.json` (defaults),
`auth.json` (credentials, see below), `sessions/` (session files) and
`prompts/` (templates) — and the sidecar's extension discovery follows the
upstream three-stage order: (1) `<project>/.pi/extensions/`, (2)
`<agent-dir>/extensions/`, (3) explicit `--extension-dir` paths (deduped by
resolved path), so your global extensions in `~/.pi/agent/extensions/` load.
Run with `--isolated` for a fresh temp agent dir instead (the M1 hermetic
behavior — used by all tests). Individual extensions can of course still
access `~/.pi` themselves (the sidecar is a regular Node process).

### Sessions

Session files use the **upstream JSONL v3 format** (header +
`id`/`parentId` entry tree; `message`, `session_info`, `custom`,
`thinking_level_change`, `model_change` entries), so pi-rust sessions open in
pi and pi sessions load in pi-rust — proven by a test that feeds a
pi-rust-written file to the original `SessionManager`. Default location:
`<agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl` (upstream layout).
- `--session-file <path>` — explicit location
- `--resume <file|id>` — continue an existing session (same id)
- `--fork <file|id>` — new session with `parentSession` = old id
- `--list-sessions` — NDJSON listing (id, name, cwd, updated, entries)

### Credentials (M2 + M3)

Provider keys resolve in the verified upstream order: `auth.json` stored
credential (literal or `${ENV}` reference) → ambient env var
(`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, ...) → typed error naming the env var.
Key material never appears in logs or error text.

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
cargo test                 # 7 e2e integration tests + 7 provider/unit tests
node sidecar/smoke.mjs     # 42 sidecar protocol assertions (fake core)

# 3. Run a scenario:
source $HOME/.cargo/env    # if rustup is not on PATH
cargo run -- --extension-dir upstream/packages/coding-agent/examples/extensions/hello.ts \
  --scenario scenario.json
```

### CLI

```
pi-rust --extension-dir <path> (repeatable)
        [--agent-dir <path>]          # default: PI_CODING_AGENT_DIR or ~/.pi/agent
        [--isolated]                  # fresh temp agent dir (hermetic runs)
        --scenario <scenario.json>    # one of --scenario / --prompt / --template
        | --prompt <text>             # one of --scenario / --prompt / --template
        | --template <name> [args...] # expand a prompt template into the prompt
        [--image <path>...]           # attach images to the prompt (png/jpeg/gif/webp)
        [--session-file <path>]       # default: <agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl
        [--resume <file|id>]          # continue an existing session
        [--fork <file|id>]            # branch a new session (parentSession header)
        [--list-sessions]             # NDJSON listing, then exit
        [--command <name> [args...]]  # repeatable; run after ready, before the runs
        [--provider <id>]                 # fake|anthropic|google|google-vertex|azure|cloudflare-workers-ai|cloudflare-ai-gateway|amazon-bedrock|openai|groq|cerebras|deepseek|
                                                 #   together|huggingface|openrouter (default: settings/fake)
        [--model <id>]                # default: settings defaultModel
```

Env: `PI_RUST_SIDECAR` (sidecar entry override), `PI_RUST_NODE` (node
binary), `PI_CODING_AGENT_DIR` (agent dir override, upstream name). The
sidecar is found by walking up from the cwd, then from the executable's
directory.

### Prompt templates (M3)

`<agentDir>/prompts/*.md` and `<cwd>/.pi/prompts/*.md` (project wins on
name collisions). `--template <name> [args...]` expands the template into
the prompt: `$1`..`$9`, `$@`/`$ARGUMENTS`, `${N:-default}` /
`${ARGUMENTS:-default}`, one pass (ported from upstream `prompt-templates.ts`;
recorded v0 deviations in `roadmap/m3-config-and-state/wp-307-prompt-templates.md`).

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

### Real model providers

`--provider openai` speaks the OpenAI-compatible `chat/completions` endpoint
(streaming SSE, tool calls by `index`), `--provider anthropic` speaks the
native `messages` endpoint (`content_block_*` events, `tool_use` blocks).
Credentials come **only** from environment variables; nothing reads keys from
config files:

| Provider  | Key env | Base URL env | Default base URL |
|-----------|---------|--------------|------------------|
| openai    | `OPENAI_API_KEY` | `OPENAI_BASE_URL` | `https://api.openai.com/v1` |
| anthropic | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` |

```sh
OPENAI_API_KEY=... cargo run -- --provider openai --model gpt-4o --prompt "hello"
ANTHROPIC_API_KEY=... cargo run -- --provider anthropic --model claude-sonnet-4-20250514 --prompt "hello"
```

The conversation sent to the provider is the full transcript
(system prompt incl. extension additions, user/assistant/tool results), and
extension tool results flow back as `tool` / `tool_result` messages — so
extensions with tools work unchanged against real models. `max_tokens` is
fixed at 4096 in v0. The bridge protocol, sidecar, and scenario mode are
unaffected (providers are core-side only). Provider behavior is tested
against local mock HTTP servers (no live APIs in CI).

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

The session file is in the **upstream JSONL v3 format** (see "Sessions"
above) — pi-rust sessions open in pi and vice versa.

## Repository layout

```
src/            Rust core (transport, client, model, provider, tools, session, agent, cli)
sidecar/        Node sidecar (protocol server, API shim, dialogs, session view, smoke)
tests/          e2e integration tests (spawn core + sidecar + unmodified extensions)
tests/fixtures/ test-only fixture extensions (clearly marked; not upstream code)
docs/           bridge-protocol.md — the protocol contract
odd/tasks/      feature spec + task log (S1–S5, L1–L11)
upstream/       earendil-works/pi @ 085c84e (gitignored reference clone)
```

## Next milestones (not in this PoC)

Interactive TUI (ratatui) with theme/keybinding parity, compaction, MCP
transport, upstream session-file compat, provider invocation of
`registerProvider` extensions, sidecar respawn, token/cost accounting.
