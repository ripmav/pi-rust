# Extension bridge sidecar (T3)

Node sidecar that runs the **original, unmodified upstream extension loader**
(`discoverAndLoadExtensions`) and `ExtensionRunner` from
`@earendil-works/pi-coding-agent`, and serves the **bridge protocol v0**
(see `docs/bridge-protocol.md`) as NDJSON on stdio. The Rust core (T4) is the
RPC client; this process is the RPC server.

## Runtime choice: Node v26.13.0 + built `dist/` (PRIMARY)

Verified working; the Bun 1.4.4 / TS-source fallback was **not needed**.
Node runs against the built `dist/`, where the loader's alias resolution
(`loader.ts:76-120`) maps extension imports (`@earendil-works/pi-ai`,
`typebox`, …) onto the built packages — the virtual-modules whole-index path
is Bun/SEA-only.

## Exact commands

Build upstream (from the pi-rust root; `upstream/` is gitignored — never git in there):

```sh
cd upstream
npm ci --no-audit --no-fund
npm run build        # builds all workspace packages, ends with coding-agent dist/
```

Run the sidecar (the core spawns it this way; cwd must be the project directory):

```sh
node sidecar/index.mjs --extension-dir /abs/path/ext1.ts --extension-dir /abs/path/ext2-dir [--agent-dir /abs/agent-dir]
```

- `--extension-dir <path>` (repeatable): explicit paths for the loader's
  discovery (file, or directory with `index.ts` / `pi.extensions` manifest).
  Discovery still scans `cwd/.pi/extensions` and the agent dir's `extensions/`
  (upstream default; override with `--agent-dir` to isolate, e.g. in tests).
- Optional env `PI_EXTENSION_SIDECAR_DIST` overrides the dist entry (default
  `upstream/packages/coding-agent/dist/index.js`).
- Protocol on stdout, all logs on stderr.

Run the smoke test (fake core over pipes; builds its own temp fixtures):

```sh
cd sidecar && node smoke.mjs     # or: npm run smoke
```

## What the sidecar does

- `init` (protocolVersion 1) → loads extensions with the original loader,
  binds `ExtensionRunner` (actions/context actions/provider actions), responds
  `success`, emits `ready` (tools with TypeBox schemas via the
  `extension.tools` → `wrapper.ts` → `tool-definition-wrapper.ts` path,
  commands, flags, providers, subscribed event names).
- F&F events dispatch to `runner.emit`; decision events
  (`tool_call`, `tool_result`, `context*`, `input`, `before_agent_start`,
  `user_bash`) run the upstream emitters and return combined results per the
  protocol's decision semantics (fast path `data: null`, first-block-wins,
  final input returned, combined tool_result mutation).
- `extension_tool_call` executes the wrapped extension tool
  (`AgentToolResult` back on the wire). `ctx.executeTool` and nested
  `ctx.tools` go through the `tool_call_request` bridge message (core answers;
  the sidecar wraps the result into `AgentToolCallOutcome` for the runner).
- Dialog subset (`select`, `confirm`, `input`, `editor` block on
  `extension_ui_response` with the negotiated timeout and upstream defaults on
  timeout/abort; `notify`, `setStatus`, `setTitle`, `set_editor_text` are
  F&F). `pi.sendMessage` surfaces as `notify`. Methods the core does not list
  in `capabilities.ui` resolve to their upstream default without a request.
- `pi.events` stays in-process (upstream `createEventBus`).
- Session: minimal in-memory view (name, entries buffer for
  `ctx.sessionManager` reads, last model/thinking level). The sidecar never
  writes session files; `session_append_entry`, `user_message`,
  `set_model_request`, `set_thinking_level_request` are its only state
  mutations toward the core.
- `registerShortcut`, message/entry/tool renderers, markdown transformers:
  accepted by the loader, ignored by the bridge, logged once per extension on
  stderr. `registerMcpServer` is accepted and reported in `ready` only.
- Handlers never fail a command: `extension_error` line + protocol default.
- Liveness: line dispatch is fire-and-forget (upstream rpc-mode pattern) so a
  command blocked on a dialog still lets `extension_ui_response` and
  sidecar→core responses flow (no head-of-line blocking either direction).

## Protocol resolutions (minimal, for T4 + doc amendment)

1. **`command` (core→sidecar, req/resp)** — *new, not in the protocol doc*:
   `{ id, type: "command", name, args? }` → success. Without it `ready.commands`
   is report-only and registered commands (e.g. event-bus `/emit`) are
   unreachable from the core. Looked up by invocation name, then plain name.
2. **`set_model_request`** carries `{ modelId, provider }` — the contract says
   `{ modelId }`, but model ids are not unique across providers; the core's own
   `set_model` command uses the same pair.
3. **`session_append_entry`** carries the `SessionEntry` nested under `entry`
   (`{ type: "session_append_entry", entry: <SessionEntry> }`) — inline would
   let the entry's own `type` (e.g. `"custom"`) clobber the event
   discriminator.
4. **Two-phase context**: `context` always answers `data: null` (sidecar
   transforms nothing at that point); the single upstream `emitContext` runs on
   `context_with_system`, whose response the core uses as-is for the provider
   request. Net result is identical to upstream's one-shot `emitContext`.
5. **Dialog timeout**: extensions that omit `opts.timeout` (including
   `editor`, which has none in rpc-mode) get the negotiated
   `capabilities.timeoutMs` (default 30000) instead of blocking forever.
6. **`before_agent_start`** wire shape: trigger fields under `event`, rendered
   `systemPrompt` as a top-level sibling; the response includes `systemPrompt`
   only when a handler changed it (otherwise omission, per contract).
7. **`extension_error` attribution**: `emitToolCall` does not catch handler
   throws (upstream), so a throwing `tool_call` handler yields
   `extensionPath: "<unknown>"` (the runner's per-handler catch is not
   reachable without re-implementing dispatch).
8. **`pi.setModel`** sends `set_model_request` and resolves `true`/`false` from
   the core's `{ ok }` answer; the session view updates on success and on the
   core's `set_model` echo.

## Smoke test

`smoke.mjs` spawns the real entry point and drives it as a fake core with five
**unmodified** upstream example extensions (`hello.ts`, `with-deps/`
(npm-dep proof), `permission-gate.ts`, `event-bus.ts`, `claude-rules.ts`) plus
one scratch fixture (created in a temp dir, never committed) that exercises
`sendUserMessage`/`appendEntry`/`setSessionName`/`registerFlag`. Assertions:
handshake + `ready` registry, `pi.events` round-trip, `tool_call` block via
`select` (reason + unchanged input), `extension_tool_call`
`AgentToolResult` shape, sidecar→core mutation events, command invocation,
`before_agent_start` prompt rewrite, fast paths, parse/unknown-command errors,
clean `session_shutdown` exit. 31 checks.
