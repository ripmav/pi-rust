# Changelog

User-facing release history for pi-rust, newest first. Every release tag
since the first staging release is listed. Dates are tag dates.
Entries from the first release-please release onward are generated from
conventional commits (see `README.md` → Releases).

## v1.0.0 (2026-11-21)

Milestone M11 — project trust & v1.0 sealing.

- **Project trust model** (upstream parity): untrusted `<cwd>/.pi/`
  content has **zero behavioral effect**; granting trust is an explicit,
  persistent, user-visible decision.
  - Trust decisions are stored per path in `<agentDir>/trust.json`
    (nearest-ancestor lookup: a decision for a parent directory covers
    everything below it).
  - New flags: `--approve`/`-a` (trust project-local files for this run)
    and `--no-approve`/`-na` (ignore them for this run).
  - New `defaultProjectTrust` setting: `"ask"` (default), `"always"`,
    `"never"`.
  - Interactive startup prompt when no decision is stored (5 options in
    upstream order/labels), re-openable with `/trust`; an untrusted run
    with trust-requiring resources shows a warning line in the
    transcript.
  - Headless runs without an explicit decision are **not trusted**
    (default-deny).
  - Behavior change: project skills (`<cwd>/.pi/skills`), prompts
    (`<cwd>/.pi/prompts`) and extensions (`<cwd>/.pi/extensions`) are now
    loaded **only for trusted projects** (agent-dir and
    `--extension-dir` resources are unaffected).
- **Project `.pi/` config under the gate** (trusted projects only):
  - `<cwd>/.pi/settings.json` is merged over the agent-dir settings;
    project values win per field. An invalid file is rejected with a
    logged message naming the path; the session continues with the
    global settings.
  - `<cwd>/.pi/mcp.json` project MCP servers are merged into the
    agent-dir `mcp.json` (upstream `loadMcpConfig` semantics: a full
    project entry replaces the same-named global server; an entry
    without `command`/`url`/`type` is an override limited to
    `enabled`/`exposure`/`toolExposure`).
- **Upstream pin refresh**: `earendil-works/pi` 17d6ca7 → 64a15f4
  (no behavior change for pi-rust; zero code migration).
- **v1.0 sealing**: stability contract in
  [`docs/stability-contract.md`](docs/stability-contract.md) (what v1
  freezes: CLI surface, bridge protocol v6, session file format,
  `~/.pi` layout, auth.json format) and this changelog.

## v0.2.1 (2026-11-15)

Fixed re-release of v0.2.0 — identical content, corrected version number.
The v0.2.0 acceptance check had caught `pi-rust --version` printing
`pi-rust 0.1.0` (the crate version had never been bumped); the v0.2.0
release was deleted before it could be pulled. Verify:
`pi-rust --version` → `pi-rust 0.2.1`.

## v0.2.0 (2026-11-15)

Milestone M10 — ecosystem + platform. **Retracted**: published once,
then deleted after the `--version` bug described under v0.2.1 above; the
fixed re-release is v0.2.1.

- `pi-rust auth login <provider>` — the upstream terminal login flow runs
  unmodified in a dedicated sidecar; oauth credential kinds (radius,
  kimi-coding, github-copilot, xai, meta) work end to end with
  sidecar-side token refresh (no token crosses the wire).
- `ctx.ui.custom` — descriptor pane (title/lines/list/input, live
  updates, keys, dialog-like timeout); headless keeps the upstream noOp.
- codemode hosting (`models: false`) and llama native provider capture.
- tool-search activation (tool exposure + active tool set on the bridge
  wire).
- **Windows x86_64 (msvc)** — fifth release target
  (`pi-rust-…-x86_64-pc-windows-msvc.tar.gz`, `pi-rust.exe`); the CI test
  matrix includes windows-latest.
- TUI parity pass: non-vision image UX (named-model warning, per-request
  image placeholder) and bash capture removes its temp log.
- `--version` now ships with the binary (the published v0.1.0 tag
  predates the flag).
- Advisory sweep (single-sourced upstream SHA in CI, atomic agent-dir
  creation) and CI hardening.
- Ships on upstream pin `earendil-works/pi` at 17d6ca7 (refreshed in
  v0.1.0).

## v0.1.0 (2026-11-14)

The first stable tag — milestone M9 (hardening & release) delivered, plus
the M10 upstream pin refresh.

- Release pipeline proven: 4 targets (Linux x86_64 gnu, Linux arm64 musl,
  macOS x86_64 + arm64) + `install.sh`; every artifact is smoke-checked
  in CI.
- CI: full test matrix, clippy + rustfmt as hard gates, perf bench job
  (`--bench` mode, baseline comparison) and a fresh-clone job that follows
  the README recipe on a clean container.
- Provider error UX: errors mapped by class with one automatic retry
  (401/403 → key guidance, no retry; 429 → one retry honoring
  `Retry-After`; 5xx/network → one retry); error text is redacted (never
  the key or the URL).
- Security hardening: agent dir created `0700` when pi-rust creates it;
  the bash full-output log is `0600`.
- Docs: README rewritten as the real docs; `docs/cli-reference.md` is
  diff-tested against `pi-rust --help`.
- Upstream pin refresh: `earendil-works/pi` 085c84e → 17d6ca7.

## v0.0.0-staging.8 (2026-11-14)

Pre-1.0 dev release — release-pipeline fix (the smoke runner is a command
prefix, not a binary argument). Last staging release; the pipeline is
proven for v0.1.0.

## v0.0.0-staging.7 (2026-11-14)

Pre-1.0 dev release — release-pipeline fix (the aarch64 smoke check had
run the cross binary natively; the smoke invocation is fixed).

## v0.0.0-staging.6 (2026-11-14)

Pre-1.0 dev release — release-pipeline fix (stripping cross-built
binaries is best-effort with a diagnostic; the smoke check stays the
gate).

## v0.0.0-staging.5 (2026-11-14)

Pre-1.0 dev release — release-pipeline fix (musl cross build via
`cross`; single publish job).

## v0.0.0-staging.4 (2026-11-14)

Pre-1.0 dev release — release-pipeline fix (upstream npm steps run in
the upstream dir).

## v0.0.0-staging.3 (2026-11-14)

Pre-1.0 dev release — provider error UX (errors mapped by class with one
automatic retry, redacted) + release-pipeline fix (upstream npm steps run
in the upstream dir).

## v0.0.0-staging.2 (2026-11-14)

Pre-1.0 dev release — clippy + rustfmt are now hard CI gates; releases
publish with the gh CLI.

## v0.0.0-staging.1 (2026-11-14)

First release from the new tag → release pipeline (4 targets: Linux
x86_64 gnu, Linux arm64 musl, macOS x86_64 + arm64, + `install.sh`).
Contents: the then-current feature set — interactive TUI, model
providers, built-in tools, MCP, unmodified upstream extensions, skills,
RPC mode (M1–M8).
