# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D5 | answered — in progress: "D5: Option 1 all 6 will be now core extensions" (user, 2026-11-21) | Which TS extensions get ported first (reference set)? | 1) The six bridge-matrix examples (`hello`, `with-deps`, `permission-gate`, `claude-rules`, `plan-mode`, `prompt-customizer`) — **chosen**: they already have e2e evidence to compare against; 2) A different set | option 1 — and the six ports become **core extensions** (bundled, default-loaded, per user) | E1S4 |
