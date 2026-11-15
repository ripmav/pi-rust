# pi-rust v0.2.1

The ecosystem + platform release: unmodified upstream extensions keep
gaining surface (`ctx.ui.custom` panes, oauth credentials, codemode
hosting, llama native providers, tool-search), and pi-rust now runs on
**five platforms** including Windows.

## What's new since v0.1.0

**Extension bridge (upstream pin refreshed to `17d6ca7`, 157/157 exports):**
- `ctx.ui.custom` — descriptor pane (title/lines/list/input, live updates,
  keys, dialog-like timeout); headless keeps the upstream noOp (WP-1006)
- `pi-rust auth login` — the upstream terminal login flow runs unmodified
  in a dedicated sidecar; oauth credential kinds (radius, kimi-coding,
  github-copilot, xai, meta) work end to end with sidecar-side token
  refresh — no token crosses the wire (WP-1007)
- codemode hosting (`models: false`) and llama native provider capture
  (WP-1004/WP-1005); tool-search activation (WP-1003)
- TUI parity pass vs the refreshed upstream (WP-1009): non-vision image UX
  (named-model warning, per-request image placeholder), bash capture
  temp-file hygiene

**Platforms:**
- **Windows x86_64 (msvc)** — fifth release target; CI test matrix includes
  windows-latest; install documented (no Windows installer, like upstream)
  (WP-1008)
- `--version` now ships with the binary (WP-1002)

**Hardening:** advisory sweep with per-line resolutions, CI upstream-tree
verify/repair (import oracle + self-heal), sidecar stdio chunk-safety.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | bash
# pinned version:
curl -fsSL https://raw.githubusercontent.com/ripmav/pi-rust/main/install.sh | PI_RUST_VERSION=v0.2.1 bash
```

Targets: Linux x86_64 (gnu), Linux arm64 (musl), macOS x86_64 + arm64,
Windows x86_64 (msvc, `pi-rust.exe`).
Requires bash, curl, tar and **node ≥ 22.19** (Windows: Git for Windows
for the bash tool; the TUI PTY test matrix is POSIX-only by design).

Verify: `pi-rust --version` → `pi-rust 0.2.1`.

From source: see the README ("From source (fresh clone)") — the CI
`fresh-clone` job follows that recipe in a clean container on every push.

## Known limitations (v0)

Unchanged from v0.1.0, plus: no project-level `.pi/mcp.json` (needs the
project-trust model, a v1 item — agent-dir `mcp.json` works), no
Windows ARM64, `ctx.ui.custom` stays a descriptor subset, pixel-perfect
TUI parity is not the bar (functional parity is).
