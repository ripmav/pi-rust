# WP-1206 — `pi-ext`: core target (C ABI + dlopen)

**Milestone:** M12 · **Story:** E1S2 · **Size:** M · **Status:** open · **Depends on:** WP-1203, WP-1204

## Goal
The cdylib target for core extensions ([ADR 0006](../../adr/0006-core-extensions-load-in-process-as-so.md)):
the `pi_ext_core_init_v1` entry, the `PiCoreApi` vtable, and the fault
boundary.

## Scope
- C ABI: `pi_ext_core_init_v1(const PiCoreApi*) -> int`,
  `PI_EXT_CORE_API_VERSION = 1`. `PiCoreApi` = fixed C struct of function
  pointers mirroring the bridge command surface: `register_tool`, `on`,
  `register_command`, `register_shortcut`, `register_flag` / `get_flag`,
  `ui_request`, `send_user_message`, `append_entry`, `set_active_tools`,
  `exec`, …
- JSON payloads across the boundary (the WP-1202 wire types); extension
  state behind an opaque `user_data` pointer.
- Shared runtime core: the same `Extension` API (WP-1203/1204) bound to
  either transport — stdio v6 (WP-1205) or the vtable.
- `catch_unwind` boundary around every core→extension call; a faulted
  extension is marked and skipped.
- Example cdylib + a test host that `dlopen`s it.

## Acceptance
- Test host `dlopen`s the example `.so`, registers a tool + handler,
  invokes both (JSON round-trips).
- A panicking example is caught, marked faulted, host continues; an ABI
  version mismatch → clean init failure.
- cdylib builds on all three CI targets (at minimum compiles; tests run on
  Linux).

## Out of scope
- Loading in the real core (WP-1210).
