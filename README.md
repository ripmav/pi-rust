# pi-rust

Rust port of [pi](https://github.com/earendil-works/pi) with a compatibility
layer: original pi extensions (TypeScript, unmodified) run unchanged against
the new Rust core.

**Status (M9, 2026-11):** M1–M9 delivered — interactive TUI (ratatui), real
model providers (native upstream wires), full built-in tool set, MCP
(stdio + HTTP), extension `registerProvider` invocation, compaction,
upstream session-file compat, skills, RPC mode, print/json output, perf
baseline in CI, security audit, release pipeline (4 targets + install
script). Upstream pin: `earendil-works/pi @ 17d6ca7`.

## Install

### From a release (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | bash
# or pin a version:
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | PI_RUST_VERSION=vX.Y.Z bash
```

Installs to `~/.local/share/pi-rust`, links `~/.local/bin/pi-rust` (both
overridable with `PI_RUST_INSTALL_ROOT` / `PI_RUST_BIN_DIR`). Targets:
Linux x86_64 (gnu), Linux arm64 (musl), macOS x86_64 + arm64. Requires
bash, curl, tar and **node ≥ 22.19** (the extension sidecar runs on Node;
the tarball ships the built `upstream/` tree, no npm install needed).

### From source (fresh clone)

Prerequisites: Rust stable, Node ≥ 26 (the upstream build target), npm.

```sh
git clone https://github.com/ripmav/pi-rust && cd pi-rust

# 1. Build upstream once (the sidecar runs its dist/):
mkdir upstream && git -C upstream init && git -C upstream remote add origin https://github.com/earendil-works/pi.git
git -C upstream fetch --depth 1 origin 17d6ca759a070c377147f6d76b72dab9a3ba42a3
git -C upstream checkout --detach FETCH_HEAD
cd upstream && npm ci --no-audit --no-fund && npm run build && cd ..

# 2. Build + run:
cargo build --release
./target/release/pi-rust --provider fake --prompt "hi"   # fake model, no API keys
```

The sidecar is found by walking up from the cwd, then from the executable's
directory (`sidecar/index.mjs`).

## Quickstart

Prerequisites: as above (fresh clone).

```sh
# Run the test suite (spawns real sidecar + unmodified upstream extensions):
cargo test                 # in-crate + e2e (unmodified extensions) + rpc diff + PTY + skills + cli reference
node sidecar/smoke.mjs     # sidecar protocol assertions (fake core)

# Headless one-shot with a real provider:
OPENAI_API_KEY=... cargo run --release -- --provider openai --model gpt-4o --prompt "hello"

# Interactive TUI on a TTY (no one-shot flags):
cargo run --release --

# Scenario mode with an unmodified upstream example extension:
cargo run -- --extension-dir upstream/packages/coding-agent/examples/extensions/hello.ts \
  --scenario scenario.json
```

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
- Upstream pin: `earendil-works/pi @ 17d6ca7` (shallow clone in `upstream/`,
  gitignored). The sidecar imports the built `dist/`, so **upstream must be
  built** before running (below). Bumping the pin may require re-checking the
  import surface (`discoverAndLoadExtensions`, `createEventBus`,
  `ExtensionRunner`, `wrapRegisteredTools`, `buildSystemPrompt`,
  `normalizeBuildSystemPromptOptions`).

## Extension compatibility guide

Unmodified upstream extensions run "wie gewohnt". The contract table from
the [vision doc](roadmap/00-vision.md), with the status per delivered
milestone (all milestone stacks are merged into `main`):

| Extension API | Status |
|---|---|
| `on(event)` for all catalog events | ✅ M1 |
| `registerTool` (TypeBox schemas, nested `ctx.executeTool`) | ✅ M1 |
| `registerCommand`, `registerFlag`/`getFlag` | ✅ M1 |
| `sendMessage`/`sendUserMessage`, `appendEntry` | ✅ M1 |
| session name, `exec`, `getSettings` | ✅ M1 |
| `setModel`, thinking level (gated on core ack) | ✅ M1/M2 |
| model providers: all bundled wires (table below) | ✅ M2/M4 |
| runtime `setModel` switching (bundled + extension providers) | ✅ M4/M7 |
| upstream session-file format, `--resume`/`--fork` | ✅ M3 |
| `~/.pi/agent` config, model catalog, auth storage | ✅ M3 |
| built-in tools parity (read/write/edit/bash) | ✅ M5 |
| real human dialog answers (select/confirm/input/editor) | ✅ M6 (TUI) |
| TUI: themes, keybindings, pickers, slash commands, print/json modes | ✅ M6 |
| `registerProvider` (custom providers invoked) | ✅ M7 |
| `registerMcpServer` (real MCP transport: stdio + HTTP) | ✅ M7/M8 |
| compaction events + auto-compaction | ✅ M7 |
| `registerShortcut` (wired TUI actions) | ✅ M7 |
| skills (`SKILL.md` discovery, `/skill:<name>`) | ✅ M8 |
| RPC mode (upstream NDJSON command protocol) | ✅ M8 |
| headless-safe API slice: `pi.events`, `registerMcpServer`, dialogs with upstream defaults | ✅ M1 (no-TTY runs answer with upstream defaults) |
| `ctx.ui.custom` (descriptor pane: title/lines/list/input, live updates, keys; dialog-like timeout) | ✅ M10 (TUI; headless keeps the upstream noOp — WP-707 subset, full component model stays a non-goal) |
| `auth login` (terminal login flow, upstream unmodified) + oauth credentials (login, use, refresh) | ✅ M10 (the upstream oauth providers — radius, kimi-coding, github-copilot, xai, meta — are sidecar-routed; the token refresh + rotated write-back run sidecar-side, no token crosses the wire) |

Evaluated, documented v0 limits: `ctx.ui.custom` full component model (mouse, focus trees, overlays — WP-707 decision memo in
`odd/tasks/m7-extension-completeness.md`; the descriptor subset ships in M10), bundled extensions beyond `mcp`
(WP-708), oauth credentials consumed by the core's built-in drivers (typed rejection — the
sidecar-routed models of those providers work), `AWS_PROFILE`
(typed rejection). Full out-of-scope list:
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

### CLI

The full flag reference is [`docs/cli-reference.md`](docs/cli-reference.md)
— **diff-tested**: `tests/cli_help.rs` fails if `pi-rust --help` drifts from
the doc. Quick view:

```sh
pi-rust [options]                      # TTY + no one-shot flags = interactive TUI
        --extension-dir <path>         # repeatable
        [--agent-dir <path>]           # default: $PI_CODING_AGENT_DIR or ~/.pi/agent
        [--isolated]                   # fresh temp agent dir (hermetic runs)
        --scenario <s> | --prompt <t> | --template <name> [args...]
        [--image <path>...]            # png/jpeg/gif/webp, needs --prompt/--template
        [--provider <id>] [--model <id>]
        [--command <name> [args...]]   # repeatable; after ready, before the runs
        [--session-file <p>] [--resume <file|id>] [--fork <file|id>] [--list-sessions]
        [-p | --print] | --mode text|json|rpc
        [--print-usage] [--bench]
        [-h | --help]
```

Env: `PI_CODING_AGENT_DIR` (agent dir, upstream name), `PI_RUST_SIDECAR`
(sidecar entry override), `PI_RUST_NODE` (node binary). Exit codes:
`0` clean · `1` fatal · `2` model-script/shutdown failure · `3` degraded.

### Interactive TUI (M6, WP-601)

Run `pi-rust` without `--scenario`/`--prompt`/`--list-sessions` on a TTY to
get the interactive terminal UI (ratatui + crossterm): a transcript pane,
a multi-line editor pane (bracketed paste, history with up/down, word-wise
deleting, cursor movement), and a status bar (model, cwd, session, titles
set by extensions). Type a message and press Enter to send it (shift+Enter
or ctrl+j inserts a newline; `ctrl+w`/`alt+Backspace` delete a word
backward, `alt+d` forward, `ctrl+u`/`ctrl+k` to line start/end — upstream
default bindings, rebindable in WP-606). `ctrl+d` (empty editor) or
`ctrl+c` twice exits (upstream key semantics). In TUI mode the
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
The `editor` dialog reuses the editor pane itself: the prefilled text opens
in the pane, Enter answers with the pane text, Esc cancels (WP-605).

#### Keybindings (M6, WP-606)

The TUI loads the upstream keybinding schema and golden defaults (the
pinned upstream `TUI_KEYBINDINGS` + `APP_KEYBINDINGS`, 90 actions). User
bindings live in `<agentDir>/keybindings.json` (upstream path), e.g.
`{"tui.input.newLine": "ctrl+m"}` or `"enter"` / `["enter", "ctrl+m"]`
(legacy flat names like `"submit"` migrate automatically). User bindings
replace the defaults of the bound action (an empty list unbinds it); two
user bindings on the same key are reported as a conflict (upstream
semantics — default collisions are not). An invalid action or key is an
error that lists the valid actions; the TUI then starts on the defaults
and shows the error in the status bar. In v0 the wired actions are the
editor/input/select/app core keys; picker and theme actions resolve and
are configurable now and get wired by their feature WPs.

#### Themes (M6, WP-607)

The TUI renders with the pinned upstream theme system: the built-in
`dark` and `light` themes (copied from the pinned upstream
`modes/interactive/theme/*.json`), the upstream token table (text, muted,
accent, mdHeading, mdCode, mdLink, ...), and the upstream color grammar
(`#hex`, `okhsl(...)`, `oklch(...)` — the OKLab/OKHSL→sRGB conversion is
ported from the pinned upstream `oklab.ts` and pinned against golden RGB
vectors generated from the upstream code itself). Custom themes are JSON
files in `<agentDir>/themes/` (upstream path), partial themes falling back
per token to the built-in defaults.

Select a theme with the `theme` entry in `<agentDir>/settings.json`
(`"light"`, or `"light/dark"` for the upstream auto pair — v0 takes the
dark side) or switch at runtime with the built-in command
`/theme <name>`; `pi.setTheme` from extensions (`ctx.ui.setTheme`) works
the same way in TUI mode. An unknown theme name is an error that lists
the available themes; the current theme stays and the error shows in the
status bar.

#### Slash commands (M6, WP-608)

Typing `/` in the editor opens the command menu (up to 8 rows, the
`tui.select.*` keys navigate, Enter completes the name, a second Enter
submits). The menu lists the upstream built-in command set at the pinned
commit, the discovered prompt templates (WP-307) and the extension
`registerCommand` commands (M1); an extension command whose name
conflicts with a built-in is skipped (the built-in wins, upstream
semantics).

Wired in v0: `/theme <name>` (WP-607), `/thinking <level>`,
`/model <provider/model>` (argument) and `/model` (picker, WP-609),
`/resume` (session picker, WP-609), `/session` and
`/hotkeys` (local info), `/quit`, and every prompt template + extension
command (the bridge `command` path, M1). The remaining built-ins report
`not supported in v0`. An unknown command is an error listing the
available ones. Command results render in the transcript (a set-model
success also updates the status bar).

#### Print / JSON output modes (M6, WP-610)

One-shot runs can select the upstream stdout variants:

- `pi-rust -p` / `--print` / `--mode text` — stdout is the final
  assistant text only (one trailing newline); the NDJSON event log is
  not written to stdout. Exit codes follow upstream print mode: `0`
  clean, `1` on any run failure (the error goes to stderr).
- `pi-rust --mode json` — stdout is the upstream-shaped
  AgentSessionEvent stream: the session header line, then
  `agent_start`, `turn_start`, `message_start` (user on prompt, then the
  assistant skeleton per turn), `message_update` (`text_delta` per
  streamed chunk, the cumulative `partial` snapshot dropped per upstream
  `toJsonEvent`), `message_end` (final assistant message with usage),
  `tool_execution_start`/`tool_execution_end` (toolCallId, toolName,
  args / result + isError), and `agent_end` (`messages`,
  `willRetry: false`). A failing run reports the failure in-stream as
  an error `message_end` and exits `1`.
- The default (no flag) keeps the pi-rust NDJSON event log on stdout
  (the M1+ core contract, exit codes `0`/`2`/`3` unchanged).
- stderr stays human-only in all modes.

### Prompt templates (M3)

#### Pickers (M6, WP-609)

`/model` (without an argument) opens the model picker over the bundled
registry (`provider/model` labels); selecting one switches the model
over the bridge (WP-406) and updates the status bar. `/resume` opens
the session picker (the shared `<agentDir>/sessions/` listing behind
`--list-sessions`); selecting a session resumes it in place (the core
keeps the same session handle — `Session::resume_from` swaps the file
and identity, the TUI transcript resets to the resumed messages). Both
pickers use the `tui.select.*` keys with a typed prefix filter (fuzzy
search is out of scope); Esc/ctrl+c cancel, an empty list renders a
placeholder.

### Prompt templates (M3)

`<agentDir>/prompts/*.md` and `<cwd>/.pi/prompts/*.md` (project wins on
name collisions). `--template <name> [args...]` expands the template into
the prompt: `$1`..`$9`, `$@`/`$ARGUMENTS`, `${N:-default}` /
`${ARGUMENTS:-default}`, one pass (ported from upstream `prompt-templates.ts`;
recorded v0 deviations in `roadmap/m3-config-and-state/wp-307-prompt-templates.md`).

### Skills (M8, WP-801)

`<agentDir>/skills/` and `<cwd>/.pi/skills/` are scanned for `SKILL.md`
files (a directory containing one is a skill root; otherwise direct `.md`
children of the root count too). Frontmatter `name` (falls back to the
parent directory name) and `description` are validated per the Agent Skills
spec; invalid skills are skipped with a `[core] skill: ...` warning
(stderr headless, `tui-stderr.log` in the TUI). Discovered skills are
appended to the system prompt as the upstream `<skills>` section (byte-exact
`formatSkillsForPrompt` golden), and `/skill:<name> [args]` expands to the
skill body in the TUI and in headless positional prompts (unknown skills
pass through unchanged). The model loads a skill file with the `read` tool
per the prompt hint — there is no skill activation tool. Recorded v0
deviations from upstream: `odd/tasks/m8-ecosystem.md` L2.

### RPC mode (M8, WP-802)

`--mode rpc` runs the upstream NDJSON command protocol on stdin/stdout
(the pinned upstream `modes/rpc` wire contract, verified line-for-line by a
live protocol diff against the upstream CLI,
`tests/fixtures/rpc-diff.mjs`). v0 commands: `prompt` (with
`streamingBehavior: steer|followUp` queueing while a run is active); every
other command type answers
`{success:false, error:"Unknown command: <type>"}` until a later WP adds it.
The event stream carries the full upstream `AgentEvent` set: `agent_start`,
`turn_start/end`, `message_start/end` for system/user/assistant/toolResult
messages (wire shapes with epoch-ms timestamps, usage/cost,
`rawStopReason`), streaming `message_update` (`text_start/delta/end`,
`toolcall_start/delta/end`), `tool_execution_start/update/end`, and
`agent_end` + `agent_settled` (the client idle gate). The upstream
`RpcClient` drives pi-rust unmodified
(`tests/rpc.rs::wp802_upstream_rpc_client_drives`). Recorded v0 deviations:
`odd/tasks/m8-ecosystem.md` L4/L5 (pre-queue disposition, follow-up queue
instead of mid-turn steering, `images` accepted but unmapped, remaining
commands pending).

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
pinned upstream `packages/ai` at `17d6ca7`), streams over HTTP, and is tested
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
src/            Rust core (transport, client, model, provider, tools, session, agent, cli, tui)
sidecar/        Node sidecar (protocol server, API shim, dialogs, session view, smoke)
tests/          e2e integration tests (spawn core + sidecar + unmodified extensions)
tests/fixtures/ test-only fixture extensions (clearly marked; not upstream code)
docs/           bridge-protocol.md (protocol contract), cli-reference.md (diff-tested CLI reference)
benches/        baseline.json — perf baseline for the CI bench job (WP-905)
odd/tasks/      feature spec + task log per milestone (S#/L# entries)
upstream/       earendil-works/pi @ 17d6ca7 (gitignored reference clone)
```

## Troubleshooting

- **`sidecar not found`** — the binary was run from a tree without
  `sidecar/index.mjs` (release tarballs include it). From source: run from
  the repo root or set `PI_RUST_SIDECAR`.
- **Sidecar starts, then `Cannot find module '@earendil-works/...'`** —
  upstream is not built. Run the "From source" steps 1 (`npm ci` +
  `npm run build` in `upstream/`). Release tarballs ship the built tree.
- **`node: command not found` / sidecar crash on start** — Node ≥ 22.19 is
  required (the sidecar is a regular Node process). Set `PI_RUST_NODE` to
  an explicit binary if your node is not on PATH.
- **TUI garbled on start / screen corruption** — you are inside a nested
  terminal that dropped the raw-mode restore; reopen the shell. In TUI mode
  sidecar/core diagnostics go to `<agentDir>/tui-stderr.log` (by design, so
  they cannot corrupt the screen).
- **`agent dir ... permission denied`** — pi-rust creates the agent dir
  `0700` when it does not exist (WP-906; it holds `auth.json` + sessions).
  A pre-existing dir keeps its mode — fix it with `chmod 700 ~/.pi/agent`.
- **Provider errors** (WP-904): `401/403` → check the key for that provider
  (no retry is attempted); `429` → one automatic retry honoring
  `Retry-After`; `5xx`/network → one automatic retry, then a clean typed
  error. Error text is redacted: provider name + status + clipped body,
  never the key or the URL.
- **Bench/regression noise in CI** — the CI `bench` job compares against
  `benches/baseline.json` with a per-metric band (timing/throughput 20%,
  idle RSS 50% — shared runners jitter RSS ±1.5 MB); refresh the
  baseline (re-run + commit) on toolchain bumps or new hardware, not on
  every flake.

## Roadmap status

M1–M9 delivered and merged: bridge PoC, config/state, model ecosystem,
built-in tools, interactive TUI, extension completeness (providers, MCP,
compaction, shortcuts), ecosystem (skills, RPC, print modes), hardening
& release. Full per-milestone WP lists: [`roadmap/README.md`](roadmap/README.md).

Known v0 non-goals (v1): `ctx.ui.custom` (evaluated, documented in the
M7 WP memos), non-`api_key` credential kinds (oauth, command refs — typed
rejection), Windows support (upstream targets macOS/Linux first),
upstream internal/experimental packages (`chord`, `codemode`, `durable`,
`evals`, `telemetry`), pixel-perfect TUI parity (functional parity is
the bar), multi-agent orchestration beyond pi's own model.
