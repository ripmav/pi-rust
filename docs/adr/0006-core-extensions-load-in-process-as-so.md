# 6. Load core extensions in-process as packed `.so` files

Date: 2026-11-21

## Status

Accepted

## Context

ADR 0001 chose per-extension subprocesses for Rust extensions and ruled out
`dlopen` "ever". The user then decided that **core extensions** (the six
bundled ports, [ADR 0005](./0005-bridge-matrix-ports-as-core-extensions.md))
are *packed into the release and loaded as `.so` files* — i.e. in-process.
This ADR settles D6.

The trust boundary ADR 0001 protects is **user-supplied** code (agent dir,
project `.pi/extensions`, `--extension-dir`): a malicious or buggy user
extension must never take down the core, and there crash isolation is
non-negotiable. Core extensions are **first-party** — bundled, CI-tested,
shipped with the binary, trusted by definition — and carry the same trust
level as built-in tools.

Loading core extensions in-process removes six processes and the IPC path from
the default extension set, and mirrors the existing built-in-extension
concept (the sidecar's built-in `llama`/MCP extensions run inside the Node
process — WP-1005, WP-702). The host-process-plus-cdylib variant (ADR 0001's
rejected alternative 3) would add a process and a protocol hop for no benefit
in the trusted case.

## Decision

- **Core extensions** are compiled as cdylibs (`.so`/`.dll`/`.dylib`, one per
  extension), **packed into the release alongside the binary** (a single
  install payload, unpacked at install time), and **loaded in-process** by the
  core at startup via `dlopen`; loaded by default in every session
  ([ADR 0005](./0005-bridge-matrix-ports-as-core-extensions.md)).
- **User extensions** (agent dir, project scope, `--extension-dir`) remain
  per-extension **subprocesses** speaking bridge v6
  ([ADR 0001](./0001-rust-extensions-as-bridge-subprocesses.md)), with all
  crash-isolation and degraded semantics unchanged.
- The `pi-ext` crate ([ADR 0003](./0003-workspace-with-pi-ext-crate.md))
  supports **both targets**: the v6-speaking binary (user extensions) and the
  cdylib with a core-extension init entry (core extensions), on the same API
  surface ([ADR 0002](./0002-mirror-full-upstream-extension-api.md)) — only
  the transport differs.
- This supersedes ADR 0001's "no `dlopen`/plugin loading of extension code
  into the core, ever" **for the core scope**; ADR 0001's user-extension
  subprocess model is reaffirmed here.

## Consequences

- The crash-isolation guarantee applies to **user** extensions (process
  boundary). A fault in a **core** extension can take down the core — accepted:
  first-party code gated by the full CI suite. E1S3's "an extension crash
  never kills the core" therefore scopes to user extensions; core extensions
  are instead gated by a CI dlopen test.
- No handshake, no per-peer state for core extensions: they initialize
  in-process at startup, before the first session event.
- Core `.so` files are built and shipped **with** the core binary (same
  release, same `pi-ext` version) — no cross-version skew to negotiate
  (contrast: user-extension binaries negotiate `protocolVersion`).
- E1S2: the crate must produce both artifacts per platform; CI must `dlopen`
  each core `.so` into the host and run its suite (with `catch_unwind`
  boundaries around core-extension calls as a panic safety net).
- Distribution: the install payload grows by six per-platform libraries;
  `install.sh` and CI packaging must pack, unpack and verify them.
