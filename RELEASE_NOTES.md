# pi-rust v0.1.0

First stable release. `pi-rust` is a Rust port of [earendil-works/pi](https://github.com/earendil-works/pi) with a compatibility layer: original pi extensions (TypeScript) run **unmodified** through the Node sidecar.

## What's in it

**Core (Rust):** providers (OpenAI, Anthropic, Google, Bedrock) with typed error
classification and retry, built-in tools (read, write, edit, bash, grep, find,
ls, web), sessions with upstream-compatible files, compaction, skills,
slash-commands, print/RPC modes, an interactive TUI (markdown, dialogs,
pickers, themes, keybindings), and an RPC surface for programmatic use.

**Extension bridge (sidecar on Node, upstream pinned at `17d6ca7`):**
unmodified upstream extensions load and run — custom providers, MCP servers
(stdio + HTTP/SSE), shortcuts, extension UI chrome, compaction events, the
built-in mcp extension. The unmodified-extension matrix is the standing CI
gate.

**M9 hardening:** full CI (tests, clippy/fmt hard gates, bench baseline,
fresh-clone acceptance in a clean container), release pipeline for
linux-gnu / linux-musl / darwin ×2 with smoke checks (aarch64 under QEMU),
owner-only permissions for agent-dir artifacts.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | bash
# pinned version:
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | PI_RUST_VERSION=v0.1.0 bash
```

Targets: Linux x86_64 (gnu), Linux arm64 (musl), macOS x86_64 + arm64.
Requires bash, curl, tar and **node ≥ 22.19**.

From source: see the README ("From source (fresh clone)") — the CI
`fresh-clone` job follows that recipe in a clean container on every push.

## Known limitations (v0)

- `ctx.ui.custom` is a graceful no-op (parity with upstream RPC mode); a
  descriptor pane is planned for v0.2.
- `auth.json` oauth credentials are a typed error (api_key credentials only);
  oauth support lands in v0.2.
- No Windows target yet (planned v0.2).
- TUI parity is *functional*, not pixel-perfect (ratatui vs Ink/React).
- The `pi-rust auth` command surface does not exist yet; manage
  `auth.json` by hand (upstream shape, `api_key` entries).
