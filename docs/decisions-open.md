# Open decisions — pi-rust

Decisions still open or answered but not yet recorded. Each row names what it blocks and what is assumed until
then. `Status` is `open` until the user answers; as soon as an answer arrives the row
becomes `answered — in progress` with the answer quoted, before any other work. Once the
answer is recorded (ADR, story, tenet or `AGENTS.md`), the row is deleted in that change.

| ID | Status | Decision | Options | Assumed until decided | Blocks |
|---|---|---|---|---|---|
| D9 | open | E3 scope boundary: models.json / model providers only. The sidecar's other non-js/ts-extension duties — MCP server hosting (the built-in mcp extension connects the servers sidecar-side) and the oauth builtins (WP-1007: token refresh + rotated write-back run sidecar-side by design, no token crosses the wire) — stay as-is in E3; core-nativing them (if wanted) is a separate epic/decision. | (1) E3 = providers only (assumed); (2) E3 also core-natives MCP/oauth | E3 is scoped to models.json + OpenAI-compatible providers; MCP + oauth remain sidecar-side (documented in E3 "out of scope") | E3 scope |
