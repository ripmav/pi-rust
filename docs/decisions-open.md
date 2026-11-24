# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D16 | open | Provider plugin format, load dir + scope (M15/E4, from the D15 answer 2026-11-24): the plugin ABI, the directories the core auto-loads from, and whether the compiled-in builtins (openai/anthropic/google/bedrock) also become bundled plugins (E4S3) or stay compiled-in. | (1) Rust cdylibs with a stable C ABI vtable + JSON payloads (the ADR 0005/0006 extension-runtime pattern), bundled dir shipped with the binary + `<agentDir>/providers/` user dir, builtins stay compiled-in (assumed); (2) same but builtins as bundled plugins (uniformity, E4S3 in scope); (3) another ABI (e.g. WIT/component model) | (1) until decided — E4S1 implements the loader against the C ABI vtable pattern; E4S3 stays open scope | E4S1 (ABI + loader), E4S3 scope |
