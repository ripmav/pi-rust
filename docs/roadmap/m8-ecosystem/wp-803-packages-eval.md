# WP-803 — Extension packages (npm) — evaluate

**Milestone:** M8 · **Size:** S · **Status:** done (memo) · **Depends on:** —

## Goal
Investigation + decision: upstream extension packages (installable bundles)
— mechanism, who uses it, feasibility with the Node sidecar (which can
already `npm install` for with-deps extensions). Update this file with
verdict + follow-up WPs or non-goal.

## Acceptance
- Memo in-file; no code.

## Out of scope
- Implementation.

## Memo (2026-11-13)

### Mechanism (pinned upstream @ 085c84e)
- `settings.json` → `"packages": ["npm:<spec>", "npm:<spec>@<ver>", "<local path>"]`
  (order = precedence). Resolved by `core/package-manager.ts` (~3k lines:
  spec parsing, npm install into `<agentDir>/npm` (its own `package.json` +
  lockfile), version pinning, local-path passthrough).
- A package is a directory whose `package.json` carries a `"pi"` manifest:
  `extensions`, `skills`, `themes`, `prompts` (resource dirs/entries), plus
  metadata (`image`, ...). The extension LOADER already understands package
  dirs: `discoverAndLoadExtensions` → `resolveExtensionEntries` reads the
  `pi` manifest and loads the listed extension files
  (`loader.ts:876-881`); no-manifest dirs fall back to file discovery.
- Upstream also loads package skills into the skills system and package
  themes/prompts into their respective registries (resource discovery).

### Who uses it (this machine)
`~/.pi/agent/settings.json` lists six packages — real, daily-use:
| package | resources | notes |
| --- | --- | --- |
| `npm:@dietrichgebert/ponytail` | extensions + skills | harness skill bundle |
| `npm:gentle-pi` | extensions + skills + themes + prompts | this harness |
| `npm:@topy-ai/cto-skills` | skills only | |
| `npm:gentle-engram` | extensions | memory provider |
| `npm:gentle-engram@0.7.1` | (pinned duplicate) | version-pin form |
| `../../projects/ripmav-bot` (local) | skills | `github-bot-token` skill |

All installed under `~/.pi/agent/npm/node_modules/` (lockfile present).
Every one of them is skills-centric or extension+skills; none is
extensions-only — an extensions-only slice would not serve the actual user.

### pi-rust feasibility
- **Sidecar side: already works.** The sidecar runs the ORIGINAL upstream
  `discoverAndLoadExtensions`; passing an installed package dir as an
  `--extension-dir` resolves the `pi` manifest and loads the extensions
  unmodified (same path proven for `--extension-dir` in M1–M7).
- **Missing pieces (core-only, additive):**
  1. Parse `settings.json` `packages` (pi-rust reads settings for model
     defaults; the field is currently ignored).
  2. Resolve `npm:` specs → `<agentDir>/npm/node_modules/<name>`;
     install-on-missing via `npm install --prefix <agentDir>/npm`
     (node + npm are already hard runtime deps of the sidecar).
  3. Pass resolved package dirs to the sidecar (`--extension-dir`).
  4. Scan package `skills` entries into the WP-801 skills system
     (discovery currently: `<agentDir>/skills` + `<cwd>/.pi/skills` only).
  5. Package themes/prompts: NOT in v0 (pi-rust themes are built-in since
     M6; prompt templates are core-owned) — record as follow-up scope.
- Risk: low — strictly additive discovery; no bridge protocol change;
  failure mode = package not loaded + warning (upstream behavior).

### Verdict
**Follow-up WP (size M), scheduled for M9** — not a v0 non-goal: the
mechanism is cheap for pi-rust (sidecar half already free), the user's own
configuration depends on it (5 of 6 packages are skills; the skills system
exists since WP-801), and the missing work is a well-bounded core slice
(settings field + resolver/install + two wirings). M8 stays docs-only for
this WP per its acceptance ("Memo in-file; no code").

### Proposed follow-up WP (M9)
**WP-9xx: Extension packages (core side)** — acceptance: the six packages
in `~/.pi/agent/settings.json` load unmodified (skills appear in the system
prompt, extensions register via the sidecar); install-on-missing works for
one npm spec; local-path packages work; package themes/prompts remain a
documented non-goal.
