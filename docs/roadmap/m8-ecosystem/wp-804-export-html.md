# WP-804 — Session export (HTML) — evaluate

**Milestone:** M8 · **Size:** S · **Status:** done (memo) · **Depends on:** 304

## Goal
Investigation + decision: port `core/export-html` (session → HTML) or
non-goal. Likely a small Rust template renderer; decide by upstream usage
weight.

## Acceptance
- Memo in-file; no code.

## Out of scope
- Implementation.

## Memo (2026-11-13)

### Mechanism (pinned upstream @ 085c84e, `core/export-html/`, 324K)
Template assembly, not a renderer:
- Static assets inlined into ONE self-contained HTML (no CDN, offline):
  `template.html` (2.2K), `template.css` (22.6K), `template.js` (84.6K,
  **client-side** rendering — the browser renders the session), vendored
  `marked.min.js` (43.6K), `marked-cjk-friendly.min.js` (4.6K),
  `highlight.min.js` (118.9K).
- `generateHtml`: `{{CSS}}/{{JS}}/{{SESSION_DATA}}/{{MARKED_JS}}/...`
  placeholder substitution; session data =
  `{header, entries, leafId, systemPrompt?, tools?, renderedTools?}`
  **base64-embedded**; `{{THEME_VARS}}` + derived page/card backgrounds
  from the active pi theme.
- Tools: `bash/read/write/edit/ls` are rendered by the template from JSON;
  extension tools can be pre-rendered via an optional `ToolHtmlRenderer`
  (TUI→ANSI→HTML) into a `renderedTools` map.
- Four invocation surfaces: TUI `/export [path]`, TUI `/share` (Radius /
  GitHub Gist upload — a separate feature that builds on the export), CLI
  `--export <file> [output]` (standalone `exportFromFile`), and RPC
  `export_html {outputPath?}` → `{path}`.

### Usage weight
High: a first-class session feature on four surfaces, and the upstream
`/share` workflow depends on it.

### pi-rust state
- TUI `/export` exists as an explicit stub: `"/export is not supported in
  v0"` (pinned by an in-crate test).
- The data layer is already in place: sessions are upstream v3 JSONL
  (cross-compatible), `Session::dump()` yields the entries/header/leafId
  the export embeds.
- RPC mode exists (WP-802): `export_html` is a natural command slot, and
  the protocol-diff harness would verify the response shape.
- Themes exist (M6 built-ins); mapping them to the export CSS vars is
  bounded (upstream `getResolvedThemeColors`/`getThemeExportColors`).

### Port shape (if approved)
Option A (recommended): a Rust template assembler — copy the static
assets verbatim (embedded via `include_str!`, ~190K total), replicate the
`{{...}}` substitutions and the base64 session embedding. The hard part
(client-side rendering) is **vendored, not re-implemented**. Surfaces in
scope: TUI `/export [path]`, CLI `--export`, RPC `export_html`. Out of
scope: `/share` (network upload + auth — its own WP), extension-tool
pre-rendering (needs the TUI ANSI pipeline — documented follow-up).

Size: S–M WP (one module `src/export_html.rs` + asset embedding + three
wiring points + tests: template-structure golden, base64 round-trip,
`exportFromFile` e2e on a real session, RPC e2e).

### Verdict
**Port-worthy — follow-up WP (WP-805 candidate), size S–M, M9** (or an
M8.1 add-on if the user wants it sooner). High usage weight, the data
layer already exists, zero sidecar involvement, and the renderer ships
pre-built. M8 itself stays docs-only for this WP per its acceptance.
