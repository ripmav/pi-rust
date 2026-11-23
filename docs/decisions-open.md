# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D8 | open | Where custom-provider support lives: (1) **sidecar-side** (assumed, WP-1301): the sidecar's unmodified upstream `ModelRuntime` already loads `<agentDir>/models.json` — the sidecar advertises those providers and streams them through the upstream runtime; the core is untouched. (2) **core-native**: pi-rust parses models.json in Rust and routes OpenAI-compatible providers through its built-in drivers — needed later for a Node-free run (E1S3 direction), not available until the core model registry accepts custom providers. | (1) Sidecar-side now (assumed); (2) core-native later, as a separate story when the Rust-only/Node-free goal (E1S3) lands | E2S1 implemented sidecar-side; a core-native models.json reader is explicitly NOT built in WP-1301 | E2S1 implementation |
