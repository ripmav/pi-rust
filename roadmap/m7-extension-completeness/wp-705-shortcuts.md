# WP-705 — `registerShortcut`

**Milestone:** M7 · **Size:** S · **Status:** open · **Depends on:** 606

## Goal
Extensions can register shortcuts; TUI dispatches keypresses to registered
shortcuts (precedence vs. built-in bindings per upstream).

## Acceptance
- Fixture extension registers a shortcut → pressing it in the TUI (simulated)
  fires the handler; conflict with a built-in resolves per upstream rule.

## Out of scope
- Shortcut management UI.
