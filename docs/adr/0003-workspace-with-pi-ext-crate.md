# 3. Split the repo into a Cargo workspace with a public `pi-ext` crate

Date: 2026-11-21

## Status

Accepted

## Context

Extension authors work outside this repo: E1S5 scaffolds standalone extension
projects, and E1S4 ports are reference projects. Per ADR 0001 an extension is
a binary that speaks bridge v6, so the API it codes against must be a
dependency it can declare — a module inside the binary crate cannot be
depended on usefully from outside. The API also needs its own versioning,
docs, and (later) publication path, independent of the binary's release
cycle. This ADR settles D3 (crate layout and name).

The alternative (single crate, module only) has no external authoring story
and would make E1S5 scaffolding a copy-paste exercise rather than a
`pi-ext` dependency.

## Decision

The repo becomes a **Cargo workspace**. The root crate stays the `pi-rust`
binary — all existing paths ([src/](https://github.com/ripmav/pi-rust/tree/main/src),
[tests/](https://github.com/ripmav/pi-rust/tree/main/tests), [benches/](https://github.com/ripmav/pi-rust/tree/main/benches),
[sidecar/](https://github.com/ripmav/pi-rust/tree/main/sidecar)) unchanged. A new crate at `pi-ext/` is the **public API**, named `pi-ext`:

- extension traits and types mirroring the upstream `pi` + `ctx` surface
  ([ADR 0002](./0002-mirror-full-upstream-extension-api.md)),
- the bridge v6 **wire types** — one source of truth used by the core's server
  side and the extension's client side,
- the runtime glue that turns an extension's `main` into a v6-speaking
  process (spawnable by the core, testable standalone).

## Consequences

- E1S2's acceptance is standalone: `pi-ext` builds, tests and docs itself
  (`cargo test --doc`); the E1S5 extension template depends on it by path/git.
- The shared wire types let E1S3's N-peer runtime and the extension client stop
  hand-rolling JSON in two places (the core currently constructs protocol
  payloads by hand in [src/agent.rs](https://github.com/ripmav/pi-rust/blob/main/src/agent.rs)).
- `pi-ext` is **0.x until the first stable extension API** (a stability
  contract section is added in E1S2); the binary's 1.0.x stability contract is
  unchanged.
- CI builds the workspace; the bench job and the existing test matrix apply at
  the workspace root.
