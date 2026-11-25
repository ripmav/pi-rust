# Changelog

User-facing release history for pi-rust, newest first. Every release tag
since the first staging release is listed. Dates are tag dates.
Entries from the first release-please release onward are generated from
conventional commits (see `README.md` → Releases).

## [1.2.0](https://github.com/ripmav/pi-rust/compare/v1.1.0...v1.2.0) (2026-11-25)


### Features

* core-native MCP hosting (WP-1403, E3S3) ([e5eb6b1](https://github.com/ripmav/pi-rust/commit/e5eb6b1c79238391be65ada580c5e0c22e1bf737))
* core-native models.json + OpenAI-compatible streaming (WP-1401, E3S1) ([9efadf0](https://github.com/ripmav/pi-rust/commit/9efadf00dccb1e08872c0f2356e60daff89229a4))
* core-native oauth builtins (WP-1404, E3S4) ([c8ad94f](https://github.com/ripmav/pi-rust/commit/c8ad94f8cc55259bb5c1ea548fd26e48439afbce))
* core-native oauth builtins (WP-1404, E3S4) ([14c2d73](https://github.com/ripmav/pi-rust/commit/14c2d736787fedcdb47f84cb18dff86019f3a128))
* custom providers from models.json (WP-1301, E2S1) ([4760431](https://github.com/ripmav/pi-rust/commit/47604317d8ffb12ecd5943a9862bb6e1a99337af))
* E3S5 follow-up — D11 (modelsPath: null) + D13 (registerMcpServer forward) (WP-1405) ([0088a93](https://github.com/ripmav/pi-rust/commit/0088a93b15c837e44bf0c5d03c6c2929896f5e27))
* provider plugin ABI v1 + directory loader (WP-1501, E4S1) ([3b42f27](https://github.com/ripmav/pi-rust/commit/3b42f271cda99d60ae0a08dc362bd2ba6d5e75dc))
* radius/github-copilot/xai/meta as core provider plugins (E4S2) ([a8f5da3](https://github.com/ripmav/pi-rust/commit/a8f5da34368c3805db0f840b7a5a35f91bbed977))
* radius/github-copilot/xai/meta as core provider plugins (WP-1502, E4S2) ([2747c63](https://github.com/ripmav/pi-rust/commit/2747c63bb3241afd28b6698180f39c387e77c1ce))


### Bug Fixes

* c_char-independent CStr reads (u8 &lt;-&gt; c_char placeholder cast) ([cc0cd2d](https://github.com/ripmav/pi-rust/commit/cc0cd2d650c0644abecb64bccea707002cf8f69b))
* c_char-independent FFI pointers + platform cdylib exts in plugin tests ([44c0920](https://github.com/ripmav/pi-rust/commit/44c092092e82052d0e5a9fba7e188dcad418e4d9))
* convert extension custom messages before every LLM call (upstream parity) ([cf88e16](https://github.com/ripmav/pi-rust/commit/cf88e165399917de40137e94e3ffca57b684f971))
* convert extension custom messages before every LLM call (upstream parity) ([26ed9ba](https://github.com/ripmav/pi-rust/commit/26ed9ba894f13bfa22cd352b53f4af54d43ac94c))
* e2e plugin cdylib name — Windows has no lib prefix ([97bfd5e](https://github.com/ripmav/pi-rust/commit/97bfd5ef85197964a142cd1bffeb27a7b1330271))
* e2e plugin cdylib uses the platform extension (.so/.dylib/.dll) ([a4e063f](https://github.com/ripmav/pi-rust/commit/a4e063febc29cdc0544b83c7bd29bf72682effe8))
* image-capability e2e no longer depends on models.dev drift ([90e2c2c](https://github.com/ripmav/pi-rust/commit/90e2c2c2a008f2a9b2ab11972b0279c68814d1d1))
* keep idle RSS at the WP-905 baseline (lazy effective registry) ([4122b7e](https://github.com/ripmav/pi-rust/commit/4122b7e37fb04f8334981a49bc5beb47a1b205f6))
* models.json review fixes — upstream defaults, E3S2 pin, dead field, shared string-skip (WP-1402) ([0a1c238](https://github.com/ripmav/pi-rust/commit/0a1c238b59adee77348696c8199dff202ea367a7))
* point GitHub releases at their tags so release-please can compute commits ([7fe77cc](https://github.com/ripmav/pi-rust/commit/7fe77cc6262de63b37991d52aea1f7d62d30ee73))
* re-bundle stale upstream catalog in CI (WP-1403 incident) ([8a0bc77](https://github.com/ripmav/pi-rust/commit/8a0bc77b9481fd7a370f7ef6a891a1b5275a71a5))
* re-point the registry drift test at the committed catalog (D14) ([f669bca](https://github.com/ripmav/pi-rust/commit/f669bcab2047ce0e56f0001af4b64147898d200f))
* rebase onto main — task log line break (L17/L18) + duplicate serde import ([1eda58a](https://github.com/ripmav/pi-rust/commit/1eda58a5cf11b885ee0de98b37d84927a775f17d))
* refresh-catalog step dies under set -e/pipefail on cold builds ([32f4cae](https://github.com/ripmav/pi-rust/commit/32f4cae77afbf3d936735a7f45f17d34e0a830ab))
* **release-please:** manifest root component is "", not "." ([b524403](https://github.com/ripmav/pi-rust/commit/b524403e5300548af6b8052ecccb7985eca32900))
* **release-please:** manifest root component is empty string, not "." ([7909d4d](https://github.com/ripmav/pi-rust/commit/7909d4d500d7ce445063415b059d88e16c907c73))
* user-override fixture uses the platform cdylib extension ([d71259b](https://github.com/ripmav/pi-rust/commit/d71259bc01506d8f34220331e9cbbe021e1fc3a4))

## v1.1.0 (2026-11-21)

Milestone M12 (E1S2, 3/6) — Rust extension platform groundwork. No
user-facing core behavior changes.

- **`pi-ext` framework crate** (new workspace member, ADR 0003):
  - WP-1201 — Cargo workspace + crate skeleton.
  - WP-1202 — `pi-ext::wire`: bridge protocol v6 wire types (single
    source of truth; all catalog commands/events, init/ready payloads,
    LLM message/content types, dialogs, panes).
  - WP-1203 — `pi-ext::pi`: `Extension` trait (init + 22 catalog event
    handlers), `Pi` object (messages, session name/label, exec,
    settings, setModel, thinking level), `Tool` registration +
    JSON-schema helpers, `ExtensionTransport` hook for the upcoming
    transports. Doc examples compile verbatim.
  - Note: `pi-ext` is version-locked with the core (both `1.1.0`) while
    it is unpublished; independent versioning is a follow-up.
- Docs — Epic E1 ADRs (0001–0006), `docs/architecture.md`, per-WP task
  index + status files.
- Release tooling — release-please (PR #155, #156): conventional
  commits on main open a release PR; merging it cuts the tag + GitHub
  release. This v1.1.0 is the last hand-cut release.

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
