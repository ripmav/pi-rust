# pi-rust roadmap

Full implementation of [earendil-works/pi](https://github.com/earendil-works/pi)
in Rust, with a compatibility layer so existing extensions/plugins keep working
**unmodified**.

## How to read this

- [`00-vision.md`](00-vision.md) — goal, architecture, the compatibility
  contract, principles, non-goals.
- [`01-milestones.md`](01-milestones.md) — milestone overview (M1–M9) with
  status, ordering, and dependencies.
- `m*/wp-*.md` — **one file per work package (WP)**. Each WP is small by
  design: the unit that becomes one GitHub issue and one (or a few) PRs.

## Work-package format

Every WP file states: goal, scope, acceptance criteria, out-of-scope, size,
dependencies, status. When a WP is picked up, it becomes a GitHub issue
(label `status:approved` first) and a `feat/<wp-id-slug>` branch; the WP file
is updated with the issue/PR numbers as evidence.

## Size legend

| Size | Meaning |
|------|---------|
| S | ~1 commit unit (a couple of hours of focused work) |
| M | 2–5 commit units (about a day) |
| L | several days — split into sub-WPs when planning starts |

## Status legend

`done` (linked commit/PR) · `open` · `deferred` (reason stated) ·
`non-goal` (explicitly out of scope for v1).

## Current state

- **M1 extension bridge (PoC) — done** (PR #2): Node sidecar runs the
  original upstream extension loader over an NDJSON bridge; six unmodified
  upstream example extensions validated end-to-end.
- **M3 `~/.pi` & session state — done** (PR #18, pending merge): agent
  config from `~/.pi/agent`, bundled model registry, upstream auth order,
  upstream v3 session files (cross-checked against the original
  SessionManager), `--list-sessions`/`--resume`/`--fork`, default agent dir
  `~/.pi/agent` + `--isolated`, prompt templates.
- **M2 real model providers — done** (PR #4): OpenAI-compatible
  `chat/completions` and Anthropic native `messages`, streaming, tool calls,
  env-only credentials, mock-server tests.
- Everything below M4 is `open`. (M3 delivered via PR #18, which also carries M2 into main; after the merge, PR #10 is superseded.)
