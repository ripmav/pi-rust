# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D1 | open | How are Rust extensions loaded into the core? | 1) Per-extension subprocess speaking the existing NDJSON bridge protocol — recommended: reuses the tested v6 protocol, crash-isolated, cross-platform, and the core's view stays uniform (any number of bridge peers); 2) In-process shared-library plugin (CDYLIB + C ABI via libloading): zero IPC, but fragile ABI, an extension crash kills the core, platform packaging pain; 3) Compile-time static registration: simplest, but no drop-in extension — every addition needs a rebuild | none — no implementation until decided | E1S1 (ADRs), E1S2, E1S3 |
| D2 | open | Which API surface does the Rust interface expose in v1? | 1) Full mirror of the upstream `pi` + `ctx` API, 1:1 with the bridge v6 catalog — recommended: porting becomes a drop-in and the protocol already carries everything; 2) Headless-safe slice first, TUI parts (renderers, custom pane, keybindings) in a later story | full mirror (1) | E1S2, E1S4 |
| D3 | open | Framework crate: layout and name | 1) Cargo workspace: `pi-rust` binary crate + a public API crate `pi-ext` — recommended: extension authors live outside this repo, so the API must be a standalone, dependable crate (name alternative: `pi-extension`); 2) Single crate, module only: no external authoring story | 1) | E1S2, E1S5 |
| D4 | open | Where are Rust extensions discovered, and what does one look like on disk? | 1) Reuse the existing locations (agent dir, `<cwd>/.pi/extensions`, `--extension-dir`) with a marker for the Rust kind (subdirectory or platform binary/`.so`) and the same trust gate — recommended; 2) Separate configuration for Rust extensions | 1) | E1S3, E1S5 |
| D5 | open | Which TS extensions get ported first (reference set)? | 1) The six bridge-matrix examples (`hello`, `with-deps`, `permission-gate`, `claude-rules`, `plan-mode`, `prompt-customizer`) — recommended: they already have e2e evidence to compare against; 2) A different set | 1) | E1S4 |
