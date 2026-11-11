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

### Interactive TUI (M6, WP-601)

Run `pi-rust` without `--scenario`/`--prompt`/`--list-sessions` on a TTY to
get the interactive terminal UI (ratatui + crossterm): a transcript pane,
a one-line input, and a status bar (model, cwd, session). Type a message
and press Enter to send it; `ctrl+d` (empty editor) or `ctrl+c` twice
exits (upstream key semantics; rebindable in WP-606). In TUI mode the
sidecar's stderr and the core's own diagnostic lines go to
`<agentDir>/tui-stderr.log` so they cannot corrupt the screen. The headless
NDJSON-on-stdout contract is unchanged for scenario/prompt runs.

#### Extension dialogs in the TUI (M6, WP-604)

When an extension asks the human for input (`ctx.ui.select`, `ctx.ui.confirm`,
`ctx.ui.input`, `ctx.ui.editor`), the TUI renders the dialog over the
transcript and the answer flows back over the unchanged bridge protocol
(`extension_ui_response`). Key handling: select — `↑`/`↓` + Enter, Esc
cancels; confirm — `y`/`n` (Enter = yes, Esc cancels); input/editor —
type + Enter, Esc cancels. Timeouts keep the sidecar-side semantics
(upstream defaults: select/input/editor → undefined, confirm → false).
Fire-and-forget UI methods render too: `notify` becomes a transcript line,
`setStatus`/`setTitle` show in the status bar, `set_editor_text` fills the
input. Headless/no-TTY runs keep answering with the upstream defaults.

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

Every provider below speaks its **native upstream wire** (mirrored from the
pinned upstream `packages/ai` at `085c84e`), streams over HTTP, and is tested
against local mock servers (no live APIs). The conversation sent is the full
transcript (system prompt incl. extension additions, user/assistant/tool
results), so extensions with tools work unchanged against real models.

| Provider | Wire | Credential env | Extra env |
|---|---|---|---|
| `anthropic` | `messages` SSE (`content_block_*`, `tool_use`) | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL` |
| `openai` | `chat/completions` SSE | `OPENAI_API_KEY` | `OPENAI_BASE_URL` |
| `groq` `cerebras` `deepseek` `together` `huggingface` `openrouter` | `chat/completions` SSE (upstream defaults per family) | `<PROVIDER>_API_KEY` | `<PROVIDER>_BASE_URL` |
| `google` | `generateContent` SSE (text + `functionCall`) | `GEMINI_API_KEY` | `GOOGLE_BASE_URL` |
| `google-vertex` | `generateContent` SSE (API key auth) | `GEMINI_API_KEY` | `GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION` |
| `azure` | `chat/completions?api-version`, `api-key` header | `AZURE_OPENAI_API_KEY` | `AZURE_OPENAI_BASE_URL` or `AZURE_OPENAI_RESOURCE_NAME`, `AZURE_OPENAI_API_VERSION`, `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` |
| `cloudflare-workers-ai` | `chat/completions` at `.../accounts/<acct>/ai/v1` | `CLOUDFLARE_API_KEY` | `CLOUDFLARE_ACCOUNT_ID` |
| `cloudflare-ai-gateway` | `chat/completions` at `.../<acct>/<gw>/compat` | `CLOUDFLARE_API_KEY` | `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_GATEWAY_ID` |
| `amazon-bedrock` | `converse-stream` (manual SigV4, AWS event-stream) | `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` | `AWS_REGION` (default `us-east-1`), `AWS_SESSION_TOKEN`; `AWS_PROFILE` is a typed v0 rejection |

Default base URLs come from the upstream catalog (e.g. openai
`https://api.openai.com/v1`, google
`https://generativelanguage.googleapis.com/v1beta`).

Credentials resolve **stored first**: `<agentDir>/auth.json`
(`{"<provider>":{"type":"api_key","key":"... | ${ENV_VAR}"}}`) beats ambient
env vars (M3/WP-303, matching upstream "a stored credential owns the
provider"). Non-`api_key` credential kinds (oauth, command refs) are typed
rejections in v0.

Not supported in v0 (typed error naming the upstream wire API): `xai`
(OpenAI Responses), `minimax` (Anthropic Messages endpoint variant),
`mistral` (conversations API), `fireworks` (custom API), `openai-codex`
(ChatGPT backend).

Extras:

- `--image <path>` (repeatable) attaches png/jpeg/gif/webp images to the
  prompt; the model must declare image input in the bundled registry.
  OpenAI-compatible wires send `image_url` data-URL parts, Anthropic sends
  base64 image sources.
- `--print-usage` emits a cumulative `usage` event (input/output tokens,
  `costUsd` from the bundled registry) before the transcript; per-turn usage
  entries land in the session file.
- Extensions can switch models at runtime with `ctx.setModel`
  (`set_model_request` → provider + credential re-resolution →
  `model_change` session entry).

```sh
OPENAI_API_KEY=... cargo run -- --provider openai --model gpt-4o --prompt "hello"
ANTHROPIC_API_KEY=... cargo run -- --provider anthropic --model claude-sonnet-4-20250514 --prompt "hello"
```

The bridge protocol, sidecar, and scenario mode are unaffected (providers are
core-side only).

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
