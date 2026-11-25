# Provider plugin e2e (E4S1 / WP-1501)

Test files: `tests/provider_plugin.rs`
Issue: WP-1501 (story E4S1, M15)

One scenario (`e2e_provider_plugin`) with four acceptance steps; the sidecar
is degraded in every run (spawn failure via `PI_RUST_NODE=/nonexistent/...`),
so all paths are Node-free. The sample plugin
(`plugin-samples/openai-compat/`, provider `sample-compat`) is the fixture —
the test builds it on demand and places it in the bundled dir
(`<exe dir>/providers/`).

| ID   | Scenario                                   | Input / setup                                                                                                   | Expected result                                                                                                        | Test name              |
| ---- | ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ---------------------- |
| T-01 | Plugin login (core-driven)                 | `auth login sample-compat --no-browser`; the e2e GETs the mock loopback URL the login prints                       | exit 0; `auth.json` written by the core: `{"sample-compat":{"type":"api_key","key":"mock-key-..."}}`                   | `e2e_provider_plugin`  |
| T-02 | Plugin stream, selectable like a built-in  | `--provider sample-compat --model bundled-model --prompt hi` against a mock OpenAI SSE endpoint                    | exit 0; sidecar-degraded log asserted; `provider plugin loaded` log asserted; transcript carries the mock's text        | `e2e_provider_plugin`  |
| T-03 | Core-resolved credential reaches the wire  | same run; the mock records the `authorization` header                                                             | header `bearer mock-key-...` (the auth.json credential, core-resolved — the plugin only consumes it)                   | `e2e_provider_plugin`  |
| T-04 | Failing plugins: typed log, run continues  | user dir `providers/` gets a garbage `.so` + a rustc-compiled cdylib with `abi_version = 999`                       | both typed log lines (`failed to load`, `abi_version mismatch`); the bundled plugin still serves the stream; exit 0    | `e2e_provider_plugin`  |
| T-05 | User-dir override of a bundled name        | the sample (variant `user`) copied into the user dir under its own file name; `--model user-model`                 | exit 0; `override ... shadows` log line                                                                                | `e2e_provider_plugin`  |
| T-06 | Shadowed bundled model is gone             | same setup; `--model bundled-model`                                                                                | exit ≠ 0; typed error `is not provided by provider plugin` (the whole plugin was replaced)                            | `e2e_provider_plugin`  |

In-crate loader/driver tests (same file's contract, `src/provider/plugin.rs`
tests, rustc-compiled stub cdylibs): missing dirs → empty set; garbage file
skipped while a good plugin loads; wrong `abi_version` skipped; user dir
shadows bundled (logged); unknown model → typed driver error; missing
credential → typed driver error; full `stream_open`/`stream_next`/
`stream_close` round-trip; `auth_start`/`auth_status` status shapes.
