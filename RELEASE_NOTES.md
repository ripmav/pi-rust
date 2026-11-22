# pi-rust v1.1.0

The Rust extension platform groundwork: the `pi-ext` framework crate —
the public API Rust extensions code against — lands in three work
packages, mirroring the upstream extension API (`pi` + `ctx`, ADR 0002).
No changes to core behavior, the CLI, the TUI or the bridge protocol
for existing users.

## What's new since v1.0.0

**`pi-ext` — the public Rust extension framework crate** (new workspace
member, ADR 0003):

- **WP-1201** — Cargo workspace + `pi-ext` skeleton.
- **WP-1202** — `pi-ext::wire`: one source of truth for the bridge
  protocol v6 line shapes — all 28 core→sidecar commands, 16
  sidecar→core events, `init`/`ready` payloads, every catalog event
  payload + decision result, LLM message/content types, dialogs and v6
  pane descriptors.
- **WP-1203** — `pi-ext::pi`: the core of the `pi` + `ctx` mirror — the
  `Extension` trait (init entry + 22 catalog event handlers), the `Pi`
  object (`send_user_message`, `append_entry`, session name/label,
  `exec`, `get_settings`, `set_model`, thinking level), `Tool`
  registration with JSON-schema helpers, and the `ExtensionTransport`
  hook the upcoming transports (v6 user process, WP-1205; core cdylib,
  WP-1206) implement. Doc examples compile verbatim
  (`cargo test --doc`).

**Docs** — Epic E1 (M12): ADRs 0001–0006, `docs/architecture.md`,
per-WP task index + status files.

**Release tooling** — release-please: conventional commits on `main`
open a release PR (version bump + changelog section); merging that PR
cuts the tag and the GitHub release (the release PR gets the same
manual review as every other PR). This v1.1.0 is cut manually as the
last hand-cut release.

Next (M12): the rest of the API surface — UI/commands/shortcuts/flags/
providers/MCP — and the catalog↔API parity test (WP-1204), then the two
transports (WP-1205/1206) and the six bridge-matrix ports as Rust core
extensions (WP-1210+).

## Upgrading from v1.0.0

Nothing to do: no flag, config, session-format or CLI changes.
