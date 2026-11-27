# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D18 | open | The Linux arm64 (musl) release target cannot load provider plugins — rustc cannot emit a cdylib for musl targets, and a static musl binary cannot dlopen at all (verified: "Dynamic loading not supported"), so the four OAuth plugin logins (radius/github-copilot/xai/meta) are unavailable in the musl artifact. How should those providers work on musl? | (a) build the musl core dynamically (needs a shared musl libc on the target system — portability regression vs the current static binary); (b) compile the four providers into the core binary for the musl target only (breaks the plugins-only architecture of D16 for one target); (c) accept: no bundled OAuth logins on musl (current state, documented) | (c) — the musl tarball ships without `providers/`; the typed "plugin not found" login path applies (same as v1.3.1) | Nothing today (documented limitation, not a regression — v1.3.1 shipped the same layout) |

Answered and recorded (2026-11-28, PO) — rows deleted per the lifecycle: **D19 → Option 1** (provider-driver breadth: all 43 bundled catalog providers become reachable eventually — recorded in E5S6), **D20 → Option 1** (integration surface: full RPC/SDK/CLI parity is a product goal — recorded in E5S3/E5S12 + new E5S14, the in-process embedding API), **D21 → Option 3** (extension-package distribution deferred — recorded in E5S8).
