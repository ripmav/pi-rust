# 1. Run Rust extensions as per-extension subprocesses over the bridge protocol

Date: 2026-11-21

## Status

Accepted

## Context

Epic E1 (../epics.md) adds a native Rust extension interface beside the Node
sidecar: new extensions written in Rust, existing TS extensions portable to
Rust, and a framework for authoring. This ADR settles D1 (how Rust extensions
are loaded into the core).

Forces:

- **Crash isolation** — extension code is untrusted in practice (loaded from
  disk, including project scope). An extension bug must never take down the
  core mid-session.
- **Version skew** — extensions are built at different times than the core and
  must keep working across core updates.
- **Authoring/testing DX** — extensions are written outside this repo and must
  be runnable and testable standalone.
- **Cross-platform** — Linux, macOS, Windows.
- **Reuse** — the bridge protocol v6 ([../bridge-protocol.md](../bridge-protocol.md))
  and the `BridgeClient` state machine ([src/client.rs](https://github.com/ripmav/pi-rust/blob/main/src/client.rs)) already implement a
  complete, tested "run extension code with crash tolerance" mechanism.

Performance is not the deciding factor: the measured anchor is ~180 µs per
blocking `before_agent_start` round-trip through the **Node** sidecar
(5,429.8/s, [benches/baseline.json](https://github.com/ripmav/pi-rust/blob/main/benches/baseline.json)), and the critical-path bridge cost per
agent turn is ~0.5 ms against 0.3–30 s of model latency. The option-1 vs
option-2 delta is ~0.5 ms/turn plus 3–10 MB per extension.

Alternatives considered:

1. **Per-extension subprocess, NDJSON bridge v6** — chosen.
2. **In-process cdylib + C ABI (libloading)** — zero IPC, but the struct layout
   *is* the ABI (no version tolerance), and crash isolation is only partially
   achievable: `catch_unwind` covers Rust panics but not memory faults or stack
   overflows, and no portable fault-catching mechanism exists
   (`sigsetjmp`/`siglongjmp` is non-portable and leaves held locks poisoned;
   Windows needs SEH). Result would be "panic-safe, fault-unsafe".
3. **Per-extension host process that dlopens a cdylib** — full isolation (the
   host dies, the core survives) plus protocol reuse, but it is option 1 with
   extra moving parts: the shipped artifact is still per-platform (host + `.so`)
   and the authoring-side benefit does not justify the machinery.
4. **Compile-time static registration** — no drop-in extensions; every addition
   requires a rebuild. Violates the epic requirement.

## Decision

A Rust extension is a **separate process**. The core spawns one process per
Rust extension and communicates over NDJSON on stdio using the bridge protocol
v6: same framing, same `init`/`ready` handshake with `protocolVersion: 6` and
capability negotiation, same message catalog, same response/event semantics.

The core's extension machinery generalizes from one bridge peer (the Node
sidecar) to **N bridge peers** (sidecar + N Rust extension processes). Per-peer
lifecycle reuses the existing `Running`/`Degraded`/`Stopped` semantics: an
extension crash or exit marks that peer dead — its tools/commands are dropped
from the model's tool set, decisions resolve to protocol defaults, the event is
logged, there is no respawn, and the core keeps running.

No `dlopen`/plugin loading of extension code into the core, ever.

## Consequences

- **Easier** — isolation is free and total (the semantics are already
  implemented and tested for the sidecar); version tolerance via
  `protocolVersion` + capabilities (unknown command → error response, never
  fatal); language-agnostic (anything that speaks v6 — future non-Rust
  extensions need no core change); standalone testability (pipe NDJSON into the
  binary — the existing e2e harness pattern); distribution = one static binary
  per platform (the existing `install.sh` pattern).
- **Harder** — the core must manage N peers (generalizing `BridgeClient` from
  one 1:1 child to a per-peer state machine is the main new runtime work, E1S3);
  3–10 MB memory + 1–10 ms spawn per extension (once per session); the
  framework crate must produce a binary that speaks v6, i.e. it is the
  protocol's *client* side, not a vtable (shapes [ADR 0003](./0003-workspace-with-pi-ext-crate.md)).
- **Follow-ups** — E1S2 (crate + standalone test harness), E1S3 (N-peer
  runtime; e2e incl. mixed sessions and crash semantics; the `--bench` loop is
  re-run against a Rust peer and the measured `bridge_events_per_sec` recorded
  in [benches/baseline.json](https://github.com/ripmav/pi-rust/blob/main/benches/baseline.json)), and [ADR 0002](./0002-mirror-full-upstream-extension-api.md), [ADR 0003](./0003-workspace-with-pi-ext-crate.md) and [ADR 0004](./0004-rust-extensions-share-discovery-and-trust.md).
