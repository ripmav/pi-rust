# pi-rust v1 stability contract

What pi-rust v1.0 freezes. This contract covers the surfaces users and
integrations rely on; anything not listed here is internal and may
change without notice, and the deprecation policy at the bottom governs
how frozen surfaces may evolve.

## CLI surface

`docs/cli-reference.md` is the source of truth — it is the exact output
of `pi-rust --help`, diff-tested by `tests/cli_help.rs` on every run.
Frozen:

- **Flags**: every option in the reference — one-shot
  (`--scenario`/`--prompt`/`--template`, `--image`), provider/model
  (`--provider`, `--model`), session (`--session-file`, `--resume`,
  `--fork`, `--list-sessions`), output (`-p`/`--print`,
  `--mode text|json|rpc`, `--print-usage`), trust (`--approve`/`-a`,
  `--no-approve`/`-na`), and misc (`--extension-dir`, `--agent-dir`,
  `--isolated`, `--command`, `--bench`, `--version`, `-h`/`--help`).
- **Subcommands**: `auth login <provider>` (account or `--api-key`;
  `--no-browser` prints URLs).
- **Exit codes**: `0` clean · `1` fatal (startup/config) · `2`
  model-script or shutdown failure · `3` degraded (sidecar crashed
  mid-run; built-ins kept working).
- **`--version` output format**: `pi-rust <major.minor.patch>` (e.g.
  `pi-rust 1.0.0`).
- **Env**: `PI_CODING_AGENT_DIR` (agent dir, upstream name), `PI_RUST_SIDECAR` (sidecar entry override), `PI_RUST_NODE` (node binary).

## Bridge protocol

`docs/bridge-protocol.md` is the source of truth: protocol version
**6**, NDJSON over stdio between the Rust core (client) and the Node
sidecar (server). The protocol is **versioned and additive-only**:

- The `init` handshake carries `protocolVersion`; the sidecar rejects a
  non-current version with an init error response and exits non-zero.
- Evolution is additive: new optional fields, commands or events are
  allowed (versions 1–6 only added — e.g. `mcpConfig` in v2,
  `keybindings` in v4, `projectTrusted` in v6); existing wire shapes
  are never removed or renamed.
- Unknown or unsubscribed event names are logged and ignored, never
  fatal, in both directions.
- A protocol **bump** (protocolVersion +1) is required for any breaking
  change.

## Session file format

The **upstream JSONL v3 format** (README § Sessions): one JSON object
per line — a header line, then an entry tree where each entry carries
`id`/`parentId`; entry types include `message`, `session_info`,
`custom`, `thinking_level_change`, `model_change` (plus `compaction`
entries from auto-compaction). pi-rust session files open in pi and pi
session files load in pi-rust — proven by a test that feeds a
pi-rust-written file to the original upstream `SessionManager`. Default
location: `<agentDir>/sessions/<encoded-cwd>/<ISO>_<id>.jsonl` (upstream layout).

## `~/.pi` layout (agent dir)

The agent dir resolves like upstream `getAgentDir`: `--agent-dir` > env
`PI_CODING_AGENT_DIR` > `~/.pi/agent` (README § "Agent dir, `~/.pi` and
extension discovery"). Default contents:

- `settings.json` — core settings (defaults, incl. `defaultProjectTrust`)
- `auth.json` — stored credentials (format below)
- `sessions/` — session files (format above)
- `prompts/` — prompt templates
- `extensions/` — user-scope extension dir (stage 2 of the upstream
  three-stage discovery; project `.pi/extensions` is stage 1, `--extension-dir` stage 3)
- `skills/` — always-scanned skill directory
- `mcp.json` — agent-dir MCP servers (upstream shape)
- `keybindings.json`, `themes/` — TUI user bindings and custom themes
- `trust.json` — per-path project-trust decisions
  (`{ "<canonical-path>": true | false | null }`, nearest-ancestor
  lookup)
- `tui-stderr.log` — TUI-mode sidecar/core diagnostics (not config)

pi-rust creates the agent dir `0700` when it does not exist (a pre-existing dir keeps its mode).

## auth.json format

A JSON object keyed by provider name. An `api_key` entry is
`{ "<provider>": { "type": "api_key", "key": "<literal> | ${ENV_VAR}" } }`
(README § Credentials). Resolution is stored-first: a stored credential
beats the ambient env var (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, …);
neither present → typed error naming the env var. Key material never
appears in logs or error text. Credentials stored by
`pi-rust auth login` (oauth kinds) are consumed sidecar-side: token
refresh and rotated write-back run in the sidecar, no token crosses the
wire.

## Deprecation policy

- **Breaking changes** to a frozen surface ship in a **MAJOR** version
  and carry a migration note in that version's release notes.
- **Non-breaking additions** (new optional flags, wire fields, entry
  types) are backward compatible.
- **Deprecation**: a CLI flag or wire/file field is announced as
  deprecated at least **one minor version** before it may be removed;
  deprecated items keep working until the removal version.
