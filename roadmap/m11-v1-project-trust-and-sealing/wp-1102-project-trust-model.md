# WP-1102 — Project trust model (parity port)

**Milestone:** M11 · **Size:** L · **Status:** implemented (verification below) · **Depends on:** 1101

## Goal
Port the upstream project-trust model (present in the pinned pi at
`packages/coding-agent/src/core/trust-manager.ts` + `project-trust.ts` —
discovered 2026-11-16, correcting the earlier "pi-rust design" framing)
to the pi-rust core: an explicit, persistent, per-path trust gate for
project-local resources in `<cwd>/.pi/`. Untrusted projects change zero
behavior vs. an empty `.pi/`; trusted projects enable WP-1103's settings
merge and project MCP config.

## Upstream mechanism (source of truth, pinned)
- **Store:** `<agentDir>/trust.json` = `{ "<canonical-path>": true | false | null }`
  (trust-manager.ts). Lookup = **nearest explicit decision** walking up
  from cwd to the root; `null` = explicitly cleared.
- **Options** (getProjectTrustOptions): "Trust" (cwd), "Trust parent
  folder (X)" (trusts parent, nulls cwd), "Trust (this session only)",
  "Do not trust" (cwd=false), "Do not trust (this session only)".
- **Resolution** (project-trust.ts `resolveProjectTrusted`, in order):
  1. CLI override (`--approve`/`-a` = true, `--no-approve`/`-na` = false).
  2. No trust-requiring resources in cwd → stored decision ?? **true**
     (no prompt; clean dirs never prompt).
  3. Extension `project_trust` event (first non-`undecided` wins;
     `remember: true` persists) — see adaptation D3 below.
  4. Stored decision (nearest ancestor) → return it.
  5. Global setting `defaultProjectTrust`: `"always"` → true, `"never"`
     → false, `"ask"` (default, settings-defaults.ts:30) → prompt.
  6. No UI (print/json/rpc) → **false** (not trusted).
  7. Interactive: select from the 5 options; persist non-session-only.
- **Gates** (upstream gate sites): project settings read/write
  (settings-manager), project MCP config (mcp extension loadConfig),
  project skills + ancestor `.agents/skills` (package-manager),
  project `SYSTEM.md`/`APPEND_SYSTEM.md` (resource-loader), project-scope
  package writes, `/trust` TUI command + in-chat untrusted warning
  (interactive-mode).
- **TUI:** `/trust` command mounts the trust selector (5 options,
  j/k navigation, saved-decision marker, "Saved trust decision: …
  Restart pi for this to take effect.").

## Decisions (pi-rust adaptation)
- **D1 — Store:** `<agentDir>/trust.json`, same JSON shape and
  nearest-ancestor lookup (port verbatim, no file lock — pi-rust is
  single-process; document the deviation).
- **D2 — Trust-requiring resources (pi-rust surface):** `settings.json`,
  `mcp.json`, `skills`, `prompts`, `extensions` under `<cwd>/.pi/`.
  (Revised 2026-11-16 after implementation: the core loads
  `<cwd>/.pi/prompts` (templates.rs) and the sidecar's upstream loader
  discovers `<cwd>/.pi/extensions` (bridge.mjs `discoverAndLoadExtensions`) —
  both project surfaces are trust-gated, matching the upstream
  TRUST_REQUIRING list for the surfaces pi-rust has.) Upstream's
  `themes`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `.agents/skills` are NOT
  loaded by pi-rust at all → not part of the trigger list.
- **D3 — No `project_trust` extension event in v0:** pi-rust loads no
  project extensions **before** trust resolution, so there is no
  untrusted project-extension handler to consult for the decision
  itself. Project extensions are loaded (after resolution) only when
  trusted (D2). The upstream event mechanism arrives with full
  extension-event parity.
- **D4 — `defaultProjectTrust` global setting** added to AgentConfig
  (`"ask"` default, `"always"`, `"never"`), same semantics.
- **D5 — CLI flags:** `--approve` / `-a` and `--no-approve` / `-na`
  (upstream names), help text upstream-verbatim.
- **D6 — TUI:** startup trust prompt (5 options, existing picker
  infrastructure) shown only when resolution reaches step 7; `/trust`
  command (persist decision, same "restart to take effect" behavior);
  in-chat warning line when untrusted + trust-requiring resources exist.
- **D7 — Headless:** print/rpc with no override + "ask" → not trusted;
  `--approve`/`--no-approve` work in all modes.
- **D8 — Skill gate (behavior change, parity fix):** `<cwd>/.pi/skills`
  is scanned today WITHOUT a gate (skills.rs, upstream-parity at the
  time). With the port, project skills load **only when trusted** —
  matching upstream (package-manager.ts:2666-2670). Untrusted projects
  with project skills: skills no longer load (documented in README +
  release notes as the trust-model change). Same change covers prompts
  and extensions with the D2 revision.

## Scope
- `src/trust.rs`: ProjectTrustStore (read/write/nearest lookup),
  `has_trust_requiring_resources(cwd)`, `resolve_project_trusted(...)`.
- `src/config.rs`: `defaultProjectTrust` setting field.
- `src/cli.rs`: `--approve`/`-a`, `--no-approve`/`-na` flags; resolution
  wired at startup before project-gated loads.
- `src/skills.rs`: project skill dir gated on trust (D8).
- `src/templates.rs`: project prompt dir gated on trust (D2 revision);
  sidecar init payload gains `projectTrusted`; bridge.mjs excludes
  project-scope extensions (`<cwd>/.pi/` paths) when untrusted
  (D2 revision; docs/bridge-protocol.md updated in lockstep).
- TUI: startup prompt + `/trust` + untrusted warning (D6).
- Tests: store (set/get/nearest-ancestor/null/clear, path canonicalization
  incl. Windows casing), resource trigger, resolution order (override /
  no-resources / stored / default always|never|ask / no-UI=false),
  skill gate trusted vs untrusted, prompt gate trusted vs untrusted,
  project-extension gate trusted vs untrusted (sidecar e2e), flag parsing.
- Docs: README (compatibility matrix: project config rows → "delivered
  (trusted projects)"), cli-reference (flags, /trust), superseding notes
  on WP-307 and WP-1009 T9a non-goal lines.

## Acceptance
- Untrusted `<cwd>/.pi/` content has zero behavioral effect (test:
  identical init with/without project files).
- Granting trust is explicit (TUI prompt, /trust, or --approve),
  persistent (trust.json), per-path with nearest-ancestor semantics.
- Headless without override + "ask" = not trusted.
- Suite green; fmt + clippy clean.

## Out of scope
- Enabling WP-1103 features (settings merge, project MCP) — same
  milestone, next WP; this WP builds the gate + the skill gate only.
- `project_trust` extension event (D3).
- Upstream's `.agents/skills`, SYSTEM.md, project extensions, package
  install gating (surfaces pi-rust does not have).

## Implementation record (2026-11-17)
- Core port by gentle-ai-worker (trust.rs store/resolution, config
  setting, CLI flags, skill gate, TUI prompt + `/trust` + warning,
  docs); D2 revision (prompts + extensions gates, bridge
  `projectTrusted` init field + project-extension filter, protocol
  doc) by the parent inline after two worker stalls on e2e runs.
- Sidecar `contextActions.isProjectTrusted` now returns the real
  decision (was a hardcoded `true` stub).
- Verification: fmt clean, clippy 0 warnings, in-crate 305, cli_help
  1, e2e 46 (incl. new project-extension gate), rpc 4, skills 3,
  trust 7, tui PTY 6, auth PTY 2 — 374/0; sidecar smoke 74/74.
- Behavior changes (README-documented): project skills/prompts/
  extensions now load only when trusted.
