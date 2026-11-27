# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

No open decisions.

Answered and recorded (2026-11-28, PO) — rows deleted per the lifecycle: **D18 → Option (a)** (musl release target: the musl core is built dynamically so dlopen works and the four plugins are bundled for musl — recorded as new story **E4S4**; interim state until then: the musl tarball ships without `providers/`, typed "plugin not found" login path), **D19 → Option 1** (provider-driver breadth: all 43 bundled catalog providers become reachable eventually — recorded in E5S6), **D20 → Option 1** (integration surface: full RPC/SDK/CLI parity is a product goal — recorded in E5S3/E5S12 + new E5S14, the in-process embedding API), **D21 → Option 3** (extension-package distribution deferred — recorded in E5S8).
