# WP-708 — Bundled extensions (mcp, codemode, llama, tool-search)

**Milestone:** M7 · **Size:** S · **Status:** done · **Depends on:** 702

## Goal
Per upstream bundled extension (`src/extensions/{mcp,codemode,llama,tool-search}`):
does it load unmodified on pi-rust (it's just an extension!), what core
capability does it need, and what's the effort to enable. One decision row
per extension.

## Acceptance
- Table in-file: per extension — loads? (matrix run), missing core
  capability, enable-WP or non-goal. No code.

## Out of scope
- Implementing their features (separate WPs if approved).

## Decision table (2026-11-12, upstream pinned 085c84e)

Framing: the bundled extensions are **inline factories**, not files — the
upstream CLI injects `builtInExtensions` at `src/main.ts:625`; the sidecar's
original loader loads files. They are load-checkable because the built files
(`upstream/packages/coding-agent/dist/extensions/<name>/index.js`) export the
same factory as default, and the sidecar hosts the mcp one already (WP-702,
synthetic-extension pattern; `dist/index.js` exports the mcp/codemode/
tool-search factories, llama is a direct dist import).

Matrix run (sidecar `--extension-dir dist/extensions/<name>/index.js`,
protocol v4 init, ready payload observed): **all 4 load unmodified,
`ready.errors` empty** — mcp additionally registers a duplicate `/mcp`
command because the sidecar already hosts the built-in (expected double
registration).

| extension | loads unmodified? | what registers | missing core capability | verdict / enable-WP |
|---|---|---|---|---|
| **mcp** | ✅ yes (already hosted, WP-702/703) | `/mcp` command, `mcp__*` tools on connect, events | `auth.provider` servers (modelRegistry stub has no `getApiKeyForProvider`); `/mcp` manager UI (ui.custom, WP-707 non-goal) | **Active now.** Edge gaps are follow-ups (OAuth = WP-703 out-of-scope line). |
| **codemode** | ✅ yes (observed: tool `codemode` registered, `defaultActive:false`, `exposure: model-only`) | one tool + TUI renderers (renderers logged ignored) | (a) model-only tool activation invisible to core (`setActiveTools`/active-set emits no wire event — shared with tool-search); (b) `ctx.modelRegistry` model calls from scripts (`getModelsOfType`/`classify`/`generateImages` missing on the stub → TypeError when used); (c) `constrainedSampling` grammar handling core-side. QuickJS-wasm + nested `ctx.executeTool` work in-process (resolvable, `tool_call_request` exists). | **Medium WP (post-v0 candidate):** host the factory + active-set wire event + model-call bridge. Until then: functions as a tools-only script runner, tool stays model-inactive. Non-goal in v0 (the v0 agent has no codemode concept). |
| **llama** | ✅ yes (observed: `/llama` command; **`ready.providers` empty** — native `registerProvider` object form is a log-only noOp in the sidecar) | `/llama` command, native provider object (not captured) | Native provider capture: WP-701 supports only the legacy config form (`{ models, streamSimple, … }` object is not retained); env-based auth context (`LLAMA_BASE_URL`/`LLAMA_API_KEY` → `getProviderAuth`); `refreshModels` → catalog population. No process spawn: it talks to an **externally running** llama.cpp router. | **Medium WP (post-v0 candidate):** extend WP-701 to capture the native Provider object (it already carries `streamSimple` → the existing `provider_stream` path drives it) + env auth. `/llama` TUI manager + HF download flow: non-goal (headless, ui.custom WP-707, no auth storage in bridge). |
| **tool-search** | ✅ yes (observed: tool `tool_search` registered, `defaultActive:false`) | one tool (`reachesTools: all`) | Same (a) as codemode: `setActiveTools` activation invisible to core (no wire event) + `exposure` not carried in the tool payload (core can't declare deferred tools). Execution itself is pure in-process BM25 over `getAllTools()`. | **Small WP (cheapest of the four):** host the factory + emit the active-set change after `setActiveTools` + carry `exposure` in the tool payload. Everything else already exists. |

Shared enabler for codemode + tool-search: the **active-set wire event**
(sidecar → core, `tools_changed`-style, after `setActiveTools`) — one small
bridge feature unlocking both. Ordering if approved: tool-search (small) →
codemode (adds the model-call bridge) → llama native provider (independent).

Out of scope per WP: implementing their features (separate WPs if approved).
