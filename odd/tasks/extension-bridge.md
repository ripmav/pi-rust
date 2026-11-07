# Feature: Extension Compatibility Bridge (PoC)

Status: planning | Upstream reference: `upstream/` (earendil-works/pi, shallow clone at session HEAD)

## Specs

- S1 (user request, verbatim): "Ich möchte dass du von folgendem Projekt ein Rust-Port erstellst: `https://github.com/earendil-works/pi` Es soll eine kompatibiltätsschicht haben um Extensions usw. vom Original weiterhin wie gewohnt nutzen zu können"
- S2 (milestone decision, verbatim option chosen): "Brücke zuerst (Proof of Concept) — Erst die Kompatibilitätsschicht validieren: API-Fläche, Reuse des originalen Loaders, JSON-RPC-Protokoll – mit minimalem Agent dahinter. Schnellster Beweis, dass 'wie gewohnt' funktioniert."

PoC success criteria (derived from S1+S2):
- S3: Unmodified upstream example extensions load and work "wie gewohnt" for the headless-safe API slice, driven by the Rust process via the Node sidecar.
- S4: The sidecar reuses the original extension loader code from upstream (no rewrite of extension loading logic).
- S5: Bridge protocol is NDJSON over stdio between the Rust core and the Node sidecar: typed command/response/event lines, reusing upstream `modes/rpc` framing (amended from JSON-RPC, see L4).

## Tasks

- T1: Explore upstream extension host (loader, ExtensionAPI, events, tool execution path, existing RPC mode) — S1,S2 — route: explore subagent (DONE, findings in L4) — commit: n/a
- T2: Define bridge protocol v0 (message catalog, error model, capability negotiation) — S2,S5 — route: worker (context backstop) (DONE, parent-amended) — commit: docs/bridge-protocol.md
- T3: Node sidecar: entry point, original loader reuse, ExtensionAPI shim, RPC server — S2,S3,S4 — route: TBD after T1 — commit: sidecar/
- T4: Rust core: sidecar spawn, RPC client, minimal agent loop, built-in tools, event dispatch — S2,S5 — route: TBD after T2 — commit: crates/
- T5: Validation: >=5 unmodified upstream example extensions (tools, commands, events, system prompt, npm-deps) run end-to-end — S3 — route: inline tests — commit: tests/
- T6: Docs: README (architecture, protocol, supported/unsupported API slice, how to run) — S2 — route: inline — commit: README.md

## Log

- L1: User request verbatim: "Ich möchte dass du von folgendem Projekt ein Rust-Port erstellst: `https://github.com/earendil-works/pi` Es soll eine kompatibiltätsschicht haben um Extensions usw. vom Original weiterhin wie gewohnt nutzen zu können"
- L2: User selected milestone "Brücke zuerst (Proof of Concept)" (see S2).
- L3: Upstream cloned to `upstream/` (shallow). `pi-rust/` is not a git repo yet: git init + feature branch required before first work-unit commit (T3).
- L4: T1 explore (subagent) done. Key facts with upstream paths: loader.ts:840-887 discovery (cwd/.pi/extensions, agentDir via PI_CODING_AGENT_DIR, configured paths); jiti-based TS loading; createExtensionAPI in loader.ts. Headless-safe API slice: on/registerTool/registerCommand/registerFlag+getFlag/sendMessage/sendUserMessage/appendEntry/session name+label/exec/getSettings/setModel+thinking level/registerProvider/MCP registration/pi.events. TUI-bound: registerShortcut, renderers, ctx.ui (noOpUIContext exists in runner.ts; rpc-mode.ts has a dialog-wired UI context). Tool path: extension.tools Map → wrapper.ts → AgentTool; hooks agent-session.ts:708-785 → runner.emitToolCall (block/mutate/terminate) / emitToolResult (mutate). Events: types.ts ExtensionEvent union (~1440-1465) incl. decision events (context, before_agent_start, input, tool_call, tool_result, turn_end, session_before_*). Precedent: modes/rpc (rpc-mode.ts:50, rpc-types.ts:24-70, 176-215) = NDJSON stdio, typed commands, extension_ui_request/response, binds full extension system headless. Loader reuse: no runtime pi-tui import in loader.ts; Node runs against built dist/ (aliases loader.ts:76-120), Bun fallback for TS source; npm-dep extensions resolve from their own node_modules. ~30 example extensions headless-safe incl. with-deps, custom-provider-*, permission-gate, plan-mode, event-bus, claude-rules, subagent. DECISION: S5 amended JSON-RPC → NDJSON (upstream's proven framing; strict JSON-RPC adds no capability).
- L5: Repo initialized: `git init` on branch `feature/extension-bridge`, .gitignore excludes upstream/, target/, .atl/.
- L6: T2 done (worker): docs/bridge-protocol.md, citations verified against clone. Upstream pinned @ 085c84e. Parent amendments after review (protocol is now the T3/T4 contract): added sidecar→core messages `session_append_entry` (core persists entries; sidecar keeps only an in-memory view), `user_message` (extension-injected user turns), `set_model_request`, `set_thinking_level_request` (extension-initiated model/state switches), `tool_call_request` (nested `ctx.executeTool`), plus session-truth and bidirectional-liveness rules. Runtimes available: Node v26.13.0, Bun 1.4.4 — T3 decides (primary: Node + built dist, fallback: Bun + TS source).
