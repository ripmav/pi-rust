# Provider plugin ABI (v1)

The contract between the pi-rust core and provider plugins (E4S1/WP-1501,
D16 option 1 — the ADR 0005/0006 in-process cdylib pattern). A provider
plugin is a **Rust cdylib** (`.so` / `.dylib` / `.dll`) exposing one entry
symbol. The core loads plugins at startup, merges their models into the
registry, drives their login handshakes, and streams through them —
**Node/sidecar-free** (a plugin stream never crosses the sidecar bridge).

Reference implementation: [`plugin-samples/openai-compat/`](../plugin-samples/openai-compat/)
(provider `sample-compat`). Core side: [`src/provider/plugin.rs`](../src/provider/plugin.rs).

## Entry symbol

```c
// NULL = not a provider plugin (the loader skips the file with a typed log line).
const struct PiProviderVtable *pi_provider_plugin_init(void);
```

The returned pointer must be stable for the process lifetime (a `static`
vtable). The core never frees it.

## The vtable (ABI v1)

`#[repr(C)]` struct, 9 function pointers. Field order and signatures are
the contract — a plugin is skipped (typed log line, run continues) when
`abi_version()` does not return `1`.

```c
struct PiProviderVtable {
    uint32_t (*abi_version)(void);                    // must return 1
    const uint8_t *(*name)(void);                     // provider name, NUL-terminated
    const uint8_t *(*list_models)(void);              // JSON array of model entries
    const uint8_t *(*stream_open)(const uint8_t *req_json);  // -> {"handle":"..."} | {"error":"..."}
    const uint8_t *(*stream_next)(const uint8_t *handle);    // event JSON, or NULL when ended
    void           (*stream_close)(const uint8_t *handle);  // idempotent
    const uint8_t *(*auth_start)(const uint8_t *req_json);   // first status JSON
    const uint8_t *(*auth_status)(const uint8_t *req_json);  // next status JSON
    void           (*dispose)(void);                          // idempotent, at shutdown
};
```

All strings crossing the boundary are **NUL-terminated UTF-8** (JSON in the
places marked). **String ownership**: a returned pointer is valid until the
next vtable call **on the same thread** — the core copies every string into
an owned `String` immediately, so plugins may reuse (or thread-local)
buffers and never allocate per call. Core→plugin strings are the same
NUL-terminated UTF-8; the core hands a `CString` buffer.

## `list_models`

A JSON **array** of registry-shaped model entries (the core's `Model`
schema, camelCase — the same shape `models.json` uses):

```json
[
  {
    "id": "bundled-model",
    "name": "Sample Bundled Model",
    "api": "openai-completions",
    "provider": "sample-compat",
    "input": ["text"],
    "contextWindow": 4096,
    "maxTokens": 512
  }
]
```

- `id` is required; the vtable `name()` is authoritative for `provider`
  (the entry's `provider` is informational and may be absent).
- `contextWindow` / `maxTokens` absent → core defaults (128000 / 16384, the
  E3S1 compaction rule).
- Bad JSON / non-array / non-registry-shaped entry → the plugin is skipped
  with a typed log line (run continues).

## Streaming (`stream_open` / `stream_next` / `stream_close`)

**Request** (`stream_open`) — core→plugin JSON:

```json
{
  "model": { "id": "bundled-model", "...": "the full registry model" },
  "messages": [ { "role": "user", "content": "hi" } ],
  "credential": { "type": "api_key", "key": "..." }
}
```

`credential` is core-resolved (auth.json). For an API-key credential the
plugin gets `{"type":"api_key","key":...}`; for an oauth credential it gets
the **full stored entry** (E4S2: `{"type":"oauth","access":...,"refresh":...,"expires":...}`
plus provider-specific extras — e.g. github-copilot's `enterpriseUrl` /
`availableModelIds`). The plugin consumes it, never stores it.

**`stream_open`** returns `{"handle": "..."}` (opaque to the core,
NUL-terminated) or `{"error": "..."}`.

**`stream_next`** blocks until the next event is ready and returns one
event JSON, or **NULL** when the stream has ended. Events:

```json
{"type": "delta", "delta": "Hel"}                                   // text delta
{"type": "done",  "turn": {"text": "Hello", "tool_calls": [],
                           "usage": {"input": 3, "output": 2,
                                     "cache_read": 0, "cache_write": 0}}}
{"type": "error", "error": "HTTP 500: ..."}                         // wire failure
```

- Exactly one terminal event: `done` or `error`, followed by NULL on the
  next `stream_next`. A stream that ends without a terminal event is a
  core-side error (`the plugin stream ended without a done event`).
- `done.turn` is the core's `AssistantTurn` JSON (`text`, `tool_calls`,
  `usage`).
- **`stream_close`** frees plugin-side stream state (idempotent — safe to
  call after the stream ended). The core always calls it.

The core drives the vtable from a blocking task (the vtable is
synchronous/blocking by contract; the core's async executor is never
parked).

## Login (`auth_start` / `auth_status`)

The core owns the login UX and `auth.json`; the plugin only performs
protocol steps. Request JSON: `{}` (empty object — start a login) or a
refresh request (below).

Status JSON (both functions):

```json
{"state": "pending", "url": "http://127.0.0.1:PORT/callback", "code": "ABCD-1234"}
{"state": "done", "credential": { "type": "api_key", "key": "..." }}
{"state": "failed", "error": "user cancelled"}
```

- `pending.url`: the core prints it (and opens a browser unless
  `--no-browser`).
- `pending.code` (optional, E4S2): the one-time code of device-code flows
  — the core prints `To continue, visit {url} and enter code {code}`
  (absent for plugins without a code). ABI-compatible extension of the
  v1 pending payload.
- `done.credential`: **the core stores it** in `auth.json`
  (`crate::auth::save_credential`) — upstream credential shape
  (`{"type":"api_key","key":...}`, `{"type":"oauth","access":...}`, …).
  The plugin must never write the credential store itself.
- The core polls `auth_status` (500 ms interval, 15 min budget) until
  `done` / `failed`.

### Refresh (E4S2)

When a run starts with a stored oauth credential inside the 5-minute
validity margin (the core's `is_expired` rule — one margin for all
providers, E3S4 semantics), the core asks the plugin to refresh it:

```json
// request to `auth_start`
{
  "action": "refresh",
  "credential": { "type": "oauth", "access": "...", "refresh": "...", "expires": 1, "...": "full stored entry" }
}
```

The plugin answers **synchronously** (no pending state):
`{"state":"done","credential":{...rotated...}}` or
`{"state":"failed","error":"..."}`. On `done` the core writes the rotated
credential back to auth.json (upstream shape, other entries preserved) and
the stream request carries the rotated entry. Plugins without a refresh
implementation answer `failed` — the run stops with a typed error pointing
at `pi-rust auth login <provider>` (no silent stale-token request).

## Error convention

One convention for the whole ABI: a failing call returns
**`{"error": "..."}`** JSON (stream_open, auth_*) or a **`{"type":"error"}`**
stream event. Bad (non-JSON) returns, NULL where JSON is expected, and
panics (core-side `catch_unwind` net, ADR 0006) are all converted into
typed core errors / log lines.

## Loader (core side)

Scan order at startup:

1. **bundled dir** — `<exe dir>/providers/` (shipped next to the binary);
2. **user dir** — `<agentDir>/providers/`.

Files: the platform cdylib extension (`.so` / `.dylib` / `.dll`), sorted
by name. A missing dir is a no-op. A provider-name collision: the later
dir wins (user shadows bundled) and the override is **logged**.

Every failure is a **typed log line** and the run continues with the
remaining providers:

- `provider plugin <path> failed to load: dlopen <path>: ... — skipped (the run continues)`
- `... failed to load: entry symbol `pi_provider_plugin_init` missing ... — skipped (the run continues)`
- `... failed to load: abi_version mismatch (plugin 999, core 1) — skipped (the run continues)`
- successful load: `provider plugin loaded: <name> (N models) from <path>`
- override: `provider plugin override: <name> (<path>) shadows the bundled plugin <prev>`

## Trust model (v0)

Plugins run **in-process** with the same trust level as compiled-in
builtins: the bundled dir ships with the binary, the user dir is the
user's own `~/.pi`. **No sandbox** — a faulty plugin can take down the
run (a memory fault is not catchable; panics are caught per ADR 0006 and
become typed errors). This is the accepted risk profile of ADR 0006.
