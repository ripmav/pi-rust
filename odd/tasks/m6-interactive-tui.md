# M6 — Interactive TUI

Milestone 6 of the pi-rust port: the interactive TUI (ratatui) with
upstream behavioral contracts (dialogs, keybindings, themes, markdown,
pickers, print mode), each WP as one PR in one GitHub stack (user
instruction 2026-11-11).

## Specs

- S1 (user, verbatim): "Arbeite autonom die Roadmap ab. Jeder Milestone wird ein Stack, wenn möglich mit main als base, und jedes WP wird ein PR im Stack. Jedes WP soll in möglichst kleine Arbeitspakete aufgeteilt werden welche dann, abgearbeitet, als commit im jeweiligen Branch/PR commited und gepushed werden."
- S2 (WP-601): TUI skeleton (ratatui). Interactive mode: chat pane (scrolling transcript), input line, status bar (model, cwd, session). `pi-rust` with no `--scenario/--prompt` enters the TUI. Acceptance: integration smoke (enter TUI, send message, scripted fake model reply rendered, q/exit works, no raw panics on resize).
- S3 (WP-602): Markdown rendering per upstream visual contract: headings, lists, code blocks with language label, inline code, bold/italic, links as text. Snapshot tests per construct; long code block scrolls within pane; CJK/emoji width test. Out of scope: syntax highlighting, math.
- S4 (WP-603): Streaming display: tokens render as they arrive, tool-call deltas show a pending indicator, no full-screen re-render per token (batch 30–60fps). Latency test: first token < 100ms after mock-stream first chunk; frame-budget sanity on a 100-token stream.
- S5 (WP-604): Human dialog answers (select/confirm/input/editor/notify/setStatus/setTitle): bridge decision events get real UI; answers flow back over the unchanged bridge protocol. E2E per dialog; timeouts per upstream timeoutMs still apply; headless (no TTY) keeps default-answers behavior. Out of scope: ctx.ui.custom (WP-707).
- S6 (WP-605): Editor pane: multiline input, bracketed paste, history (up/down), word-wise editing, `editor` dialog reuse, submit Enter / shift+enter newline (upstream keybindings). Key-sequence tests via simulation: 50-line paste (no corruption), history recall, multiline submit, input dialog reuse.
- S7 (WP-606): Keybindings: load upstream keybinding schema/defaults, configurable via `~/.pi` settings, conflict resolution per upstream. Defaults match upstream (golden); rebind test changes behavior; invalid binding → error listing valid actions.
- S8 (WP-607): Themes: upstream built-in themes + `~/.pi` custom themes dir, mapped to ratatui styles, runtime switching (TUI command + setTheme hook if present at pinned commit). Golden buffer snapshots per built-in theme; custom theme file loads; unknown theme → error.
- S9 (WP-608): Slash commands in TUI: `/…` menu = built-in commands (upstream set at pinned commit) + extension registerCommand (M1 bridge) + prompt templates (WP-307). Menu shows extension command (fixture) + template; invocation runs the bridge command path with TUI result display; unknown → error list.
- S10 (WP-609): Session & model pickers: session list (WP-305) with resume; model list (registry WP-302) with switch (WP-406). Picker tests (simulation): filter, select, enter → resume/switch asserted; empty lists render gracefully.
- S11 (WP-610): Print mode parity (see wp-610 file).

## Architecture (M6-wide)

- Testable core: `src/tui/` holds the app state, key handling, and rendering
  into a `ratatui::Buffer` — all in-process unit-testable (key injection +
  buffer snapshots), no TTY needed. A thin terminal adapter (crossterm raw
  mode, alt screen, event loop) wraps it for real runs.
- Integration smoke = drive TuiApp in-process: inject typed input + Enter →
  scripted fake model turn renders in the transcript → inject exit → clean
  state; inject Resize event → re-layout without panic.
- Terminal craft floor (gentle-ai-tui-design): dense IDE vocabulary, one
  accent role, status bar one row, no boxes-in-boxes, `…` truncation in
  fixed rails, no recompute in the render path.
- Dependencies: `ratatui 0.30.2` + `crossterm 0.29.0` (latest stable,
  named by WP-601); no other new crates.

## House rules for M6 (S1-derived)

- One GitHub stack for M6, first PR based on `main`, each subsequent WP PR
  based on the previous PR's head; every PR registered via the stacks API.
- Exactly one PR per WP; small work packages committed AND pushed as they
  complete. Branch naming `feat/wp-6NN-<slug>`; each PR links its WP issue +
  one `type:*` label; CI status reported honestly (no CI configured).
- TDD: RED before GREEN; upstream contracts mirrored from the pinned
  upstream at 085c84e (read-only reference; upstream TUI is a custom
  framework — we mirror behavior/contract with ratatui, not implementation).

## Tasks

| ID | Spec | WP | Issue | Route | Commit/PR |
|----|------|----|-------|-------|-----------|
| T1 | S2 | 601 | #47 | parent (architectural) | commits 22e14e5/e01b167/93226b8; PR (base main) |
| T2 | S3 | 602 | #48 | inline (TDD) | feat(tui) on feat/wp-602-markdown-rendering; PR #58 (stack #59) |
| T3 | S4 | 603 | #49 | inline (TDD) | feat(tui) on feat/wp-603-streaming-display; PR #60 (stack #59) |
| T4 | S5 | 604 | #50 | inline (TDD) | dce93b3/b7a850e/ec37517 on feat/wp-604-human-dialogs; PR #61 (stack #59) |
| T5 | S6 | 605 | — | subagent or inline | — |
| T6 | S7 | 606 | — | subagent or inline | — |
| T7 | S8 | 607 | — | subagent or inline | — |
| T8 | S9 | 608 | — | subagent or inline | — |
| T9 | S10 | 609 | — | subagent or inline | — |
| T10 | S11 | 610 | — | subagent or inline | — |

## Log

- L1 (user, verbatim): "Starte mit M5" / "Arbeite autonom die Roadmap ab. Jeder Milestone wird ein Stack, wenn möglich mit main als base, und jedes WP wird ein PR im Stack. Jedes WP soll in möglichst kleine Arbeitspakete aufgeteilt werden welche dann, abgearbeitet, als commit im jeweiligen Branch/PR commited und gepushed werden."
- L2: M5 delivered: stack #42 = PRs 40, 41, 43, 44, 45, 46 (WP-501..506), all
  open for user merge; main stays green (141 in-crate + 20 e2e + 42 smoke on
  the M5 branch tip).
- L3: M6 execution order by dependency: 601 → 602 → 603 → 604 → 605 → 606 →
  607 → 608 → 609 → 610.
- L4: Test strategy decided: in-process TuiApp (key injection + ratatui
  buffer snapshots) for all TUI tests; the crossterm adapter stays thin.
  This keeps the acceptance criteria (key-sequence tests, integration smoke)
  deterministic without PTY fragility.
- L6 (slice boundary): M6 delivery strategy = one GitHub PR per WP, so each
  WP commit chain is one review slice (house-rule slice unit). The cumulative
  since-main line count (incl. the ratatui/crossterm Cargo.lock bump in the
  WP-601 PR) exceeds one 400-line slice by design; slices are the PRs.
- L5 (WP-601 root causes, for later WPs): (1) the crossterm stdin reader
  thread must get a stop flag — joining it without one hangs shutdown, since
  it only notices the dropped channel on the next key event (which never
  comes on quit); (2) core `[core]` eprintln lines and the sidecar stderr
  must route to `<agentDir>/tui-stderr.log` in TUI mode — stderr text on the
  pty survives ratatui's diff rendering (unchanged cells are not rewritten)
  and corrupts the transcript rows; (3) no idle redraw tick: redraws are
  event-driven (the pending indicator changes only on agent_start/agent_end).

- L7: WP-604 delivered: T4 done, PR #61 (base feat/wp-603-streaming-display,
  label type:feature, closes #50), stack #59 = [57, 58, 60, 61]. Dialog state
  machine (src/tui/dialog.rs) + TUI render/key routing; TUI mode forwards
  dialog requests instead of the default-answers path (headless unchanged);
  F&F render (notify transcript line, setStatus/setTitle status bar,
  set_editor_text input). Root cause: --command was awaited before the TUI
  started, so a command opening a dialog would deadlock its own answer —
  commands now run on a task after the TUI loop is up (TUI mode only).
  Fixture registerCommand handler signature is (args, ctx) — first arg is
  the args string. PTY e2e asserts the full roundtrip (confirm/select/
  input) + clean exit. Suite: 98 in-crate + 19 e2e + 2 PTY; smoke 42/42.
  No CI on the repo — local suite, reported honestly.
