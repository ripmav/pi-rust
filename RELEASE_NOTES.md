# pi-rust v1.0.0

The trust + sealing release: project-local config is trust-gated like
upstream (untrusted projects have zero behavioral effect), trusted
projects can carry their own `.pi/` settings and MCP servers, and
pi-rust v1.0 freezes its surface with a stability contract.

## What's new since v0.2.1

**Project trust (upstream parity):**
- Untrusted `<cwd>/.pi/` content has **zero behavioral effect** — trust
  is an explicit, persistent, user-visible decision, stored per path in
  `<agentDir>/trust.json` (nearest-ancestor lookup: a decision for a
  parent dir covers everything below it)
- New flags: `--approve`/`-a` (trust project-local files for this run)
  and `--no-approve`/`-na` (ignore them for this run)
- New `defaultProjectTrust` setting (`"ask"` default, `"always"`,
  `"never"`)
- Interactive startup prompt when no decision is stored (5 options in
  upstream order/labels), re-openable with `/trust`; an untrusted run
  with trust-requiring resources shows a warning line; headless runs
  without an explicit decision are not trusted (default-deny)
- Behavior change: project skills, prompts and extensions under
  `<cwd>/.pi/` load **only when trusted** (agent-dir and
  `--extension-dir` resources are unaffected)

**Project `.pi/` config under the gate (trusted projects only):**
- `<cwd>/.pi/settings.json` merges over the agent-dir settings — project
  values win per field; an invalid file is rejected with a logged message
  and the session continues with the global settings
- `<cwd>/.pi/mcp.json` — project MCP servers merged into the agent-dir
  `mcp.json` with upstream `loadMcpConfig` semantics (full entries
  replace; override entries limited to `enabled`/`exposure`/
  `toolExposure`)

**Upstream pin refreshed** to `earendil-works/pi @ 64a15f4`
(consumed export matrix re-verified; no behavior change for pi-rust,
zero code migration).

**v1.0 sealing:**
- Stability contract —
  [`docs/stability-contract.md`](docs/stability-contract.md) freezes the
  CLI surface, bridge protocol v6, session file format, `~/.pi` layout
  and auth.json format, plus the deprecation policy
- `CHANGELOG.md` — full user-facing history since v0.0.0-staging.1

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | bash
# pinned version:
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | PI_RUST_VERSION=v1.0.0 bash
```

Targets (5): Linux x86_64 (`x86_64-unknown-linux-gnu`), Linux arm64
(`aarch64-unknown-linux-musl`), macOS arm64 (`aarch64-apple-darwin`),
macOS x86_64 (`x86_64-apple-darwin`), Windows x86_64
(`x86_64-pc-windows-msvc`, `pi-rust.exe`) + `install.sh`.
Requires bash, curl, tar and **node ≥ 22.19** (Windows: Git for Windows
for the bash tool; the TUI PTY test matrix is POSIX-only by design).

Verify: `pi-rust --version` → `pi-rust 1.0.0`.

From source: see the README ("From source (fresh clone)") — the CI
`fresh-clone` job follows that recipe in a clean container on every push.

## Known limitations (v1)

Not in v1.0 (next-milestone backlog): the full `ctx.ui.custom` component
model (the descriptor subset ships), non-`api_key`/oauth credential
kinds (command refs — typed rejection), ARM64 Windows, upstream
internal/experimental packages (`chord`, `codemode`, `durable`,
`evals`, `telemetry`), pixel-perfect TUI parity (functional parity is
the bar).
