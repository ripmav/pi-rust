# M10 — v0.1 & Ecosystem + Platform

Rust port of earendil-works/pi (compatibility layer: upstream extensions run
unmodified). M10 delivers: upstream pin refresh, v0.1.0, bundled-ecosystem
enablement (tool-search/codemode/llama), `ctx.ui.custom` descriptor pane,
oauth credentials, Windows target, TUI parity pass, close with v0.2.0.
One GitHub stack; first PR based on `main`, each WP = one PR based on the
previous M10 PR head.

## Specs
S1 (L2, user): "Mach M10" — autonomous milestone execution per the
established M7/M8/M9 pattern (feature doc, issues, stacked PRs, CI green,
bottom-up merge, close).
S2 (M10 planning, 2026-11-14): the WP files under
`roadmap/m10-v01-ecosystem-and-platform/` are the specification; acceptance
quoted per WP:
- S2.1 (WP-1001): "upstream/ at the new SHA; all pin references updated and
  consistent. Matrix + smoke + full suite green on the new pin. Bench
  baseline re-recorded (3-sample median); CI bench job green. Milestone doc
  records the keep/bump decision (M10 entry criterion)."
- S2.2 (WP-1002): "Release v0.1.0 published: 4 tarballs + install.sh, all
  smoke-passing in CI. Clean-container install→run verified from the real
  release artifact. Release notes published with the release."
- S2.3 (WP-1003): "Unmodified tool-search from dist/extensions loads and its
  tool activates a deferred fixture tool mid-run (e2e + smoke). exposure
  present in the tool payload (smoke check). No regression for
  always-active tools (existing matrix green)."
- S2.4 (WP-1004): "Unmodified codemode loads; its tool activates (1003 path)
  and executes a script with nested tool execution (e2e).
  getModelsOfType/classify round-trip verified; generateImages either works
  or is a documented typed sub-gap with rationale. Matrix + smoke green."
- S2.5 (WP-1005): "Native-form provider extension (unmodified llama or
  equivalent fixture) registers models in ready.providers and drives a
  scripted full turn (e2e). Env auth resolves; missing router → clean typed
  error. Legacy config form (WP-701) unchanged (regression e2e green)."
- S2.6 (WP-1006): "The two WP-707 production shapes (mcp-manager menu,
  progress pane) render, receive keys, update live, and resolve via
  done(result) (PTY e2e + smoke). Out-of-subset components degrade with a
  documented typed message (fixture test), never a crash. Existing dialogs
  (WP-604) unaffected (regression PTY green)."
- S2.7 (WP-1007): "OAuth-capable provider: auth login flow produces a valid
  auth.json entry; a run uses it; expired token refreshes per upstream
  semantics. Typed error gone for oauth credentials on supported providers;
  unsupported providers keep a precise typed error. No key/refresh material
  in any log or error (test)."
- S2.8 (WP-1008): "pi-rust.exe target built in CI and published with the
  next release; smoke (one-shot fake turn) passes on windows-latest.
  Headless e2e suite green on windows-latest. Install path (ps1 or
  documented) verified end-to-end once."
- S2.9 (WP-1009): "Per-item table in this file (gap, upstream ref,
  closed/documented, rationale). Light/dark or appearance change (if
  implemented) demonstrable in the PTY harness (theme switch visible).
  PTY + full suite green; matrix green."
- S2.10 (WP-1010): "Every advisory line has a resolution (fix + test, or
  re-documented non-goal with rationale) — none silently dropped. Suite +
  matrix + fresh-clone green at the M10 head. M10 marked delivered in
  roadmap/01-milestones.md; release notes for v0.2.0 published (if
  released)."

## Tasks
- T1 / WP-1001 — Upstream pin refresh (085c84e → origin/main) — S2.1 — issue #121 — branch `feat/wp-1001-upstream-pin` — PR #122
- T2 / WP-1002 — v0.1.0 release — S2.2 — issue #123 — branch `feat/wp-1002-v010-release` — PR #—
- T3 / WP-1003 — tool-search activation (active-set wire event) — S2.3 — issue #124 — branch `feat/wp-1003-tool-search` — PR #—
- T4 / WP-1004 — Codemode hosting (factory + model-call bridge) — S2.4 — issue #125 — branch `feat/wp-1004-codemode` — PR #—
- T5 / WP-1005 — Llama native provider capture — S2.5 — issue #126 — branch `feat/wp-1005-llama-native` — PR #—
- T6 / WP-1006 — ctx.ui.custom descriptor overlay pane — S2.6 — issue #127 — branch `feat/wp-1006-ui-custom-pane` — PR #—
- T7 / WP-1007 — OAuth credential support — S2.7 — issue #128 — branch `feat/wp-1007-oauth` — PR #—
- T8 / WP-1008 — Windows support — S2.8 — issue #129 — branch `feat/wp-1008-windows` — PR #—
- T9 / WP-1009 — TUI parity pass vs upstream p2 — S2.9 — issue #130 — branch `feat/wp-1009-tui-parity` — PR #—
- T10 / WP-1010 — M10 close (advisory sweep + verification + v0.2.0) — S2.10 — issue #131 — branch `feat/wp-1010-m10-close` — PR #—


## Log
L1 (user, 2026-11-14): "perfekt. Wo sollen wir weiter machen?" → choice:
M10 planen (planning committed as 31cc73a on main).
L2 (user, 2026-11-14): "Mach M10".
L3 (planning evidence, 2026-11-14): upstream origin/main at 8636e21 is 50+
commits behind the pin 085c84e (p2 themes/cross-fade, per-location MCP,
appearance settings, p3 docs). Pin is referenced in: README.md,
.github/workflows/{ci,release}.yml, tests/{rpc,extension_bridge}.rs,
docs/bridge-protocol.md, src/{tools,skills,session,provider/*}.rs (comments),
roadmap files. `upstream/` is a gitignored local checkout of the pin.
L4 (T1/WP-1001 evidence, 2026-11-14): pin bumped 085c84e →
17d6ca759a070c377147f6d76b72dab9a3ba42a3 (origin/main). Contract inspection
(WP-1001 file): dist/index.js export surface unchanged (157 exports, 0
added/removed), AssistantMessageEvent union unchanged, direct dist imports
(system-prompt.js, mcp-servers.js) unchanged, pi-mcp additive
(mcpServerSummary), pi-ai additive (cache-warming). Zero code migration.
Pin references bumped in README, ci.yml (incl. cache key), release.yml,
tests, docs, src comments, vision. Full suite on the new pin: 271 in-crate,
1 cli-help, 38 e2e (unmodified-extension matrix), 4 rpc, 3 PTY, 3 skills,
68 sidecar smoke — all green.
- **L5** (2026-11-14): **WP-1003 delivered** (tool-search activation, protocol v5). Sidecar hosts the
  built-in tool-search extension (dist export since pin 17d6ca7); `exposure`/`defaultActive` +
  `ready.activeTools` on the wire; upstream initial-activation rule at registration;
  `tools_activated` F&F event (coalesced with `tools_changed`); core `Core.ext_active_tools` +
  `merged_tool_defs` filters declarations to the active set (execution routing unchanged);
  `runtime.*` tool methods wired (fixes the latent mcp-ranker `getAllTools` call). Race found +
  fixed in-suite: decision responses now flush pending tool events before the response and the
  event_task select! is `biased;` (deterministic WP-702 barrier). E2E: deferred tool inactive on
  turn 1, declared + executed after mid-run `setActiveTools` (fixture tool-activation.ts,
  test-only). Full suite + 71 smoke green, fmt/clippy clean.
- **L6** (2026-11-14): slice strategy for the M10 stack: each WP is one stacked PR and one review unit (the ~400-line
slice rule applies per WP commit, not per accumulated stack; 1001: 261, 1002: 139, 1003: 423 lines on top of
main). Note: #132/#133 landed on stacked bases (GitHub stacked-merge race, same as M9) — content merged into main
via direct merge; PRs closed with explanation.

- **L7** (2026-11-14): **WP-1004 delivered** (codemode hosting). Built-in codemode extension hosted with `models: false`
(after `bindCore` — the factory needs `runtime.appendEntry`/`getSettings`); registers inactive like tool_search
(v5 activation path); sandbox (QuickJS wasm) executes scripts, nested `tools.<name>()` round-trips through the
existing `tool_call_request` wire event (zero new wire events, protocol stays v5). E2E: codemode script awaits
`tools.echo_param` + computes in the sandbox. Full suite + 72 smoke green, fmt/clippy clean. v0 cut: `models.*`
namespace (needs a model-API wire event) + prepareLoadout/reachesTools/constrainedSampling (not serialized).- **L8** (2026-11-14): **WP-1005 delivered** (llama native provider capture). Native Provider object form
captured sidecar-side (functions never cross the wire; model list via ready/providers_changed); existing
provider_stream path reused (zero new wire events); env auth resolved sidecar-side (llama: key "local");
unmodified llama hosted + refreshModels populates the catalog before ready (publish gate must call
args.update()). 3 new e2e (native turn, typed not-configured error, unmodified llama via mock router);
legacy form unchanged. Full suite + 73 smoke green, fmt/clippy clean.