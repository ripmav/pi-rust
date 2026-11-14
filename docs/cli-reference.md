# pi-rust CLI reference

This file is the exact output of `pi-rust --help` — `tests/cli_help.rs`
fails if the two ever drift (WP-907 acceptance: CLI reference matches the
actual flags, diff-tested).

```
usage: pi-rust [options]

  --extension-dir <path>    extension directory (repeatable; after the project .pi/extensions and the agent dir)
  --agent-dir <path>        agent directory (default: $PI_CODING_AGENT_DIR or ~/.pi/agent)
  --isolated                use a fresh temporary agent dir (hermetic)
  --scenario <path>         scenario JSON (one of --scenario/--prompt/--template)
  --prompt <text>           one prompt (one of --scenario/--prompt/--template)
  --template <name> [args]  expand a prompt template into the prompt (one of the three; args map to $1..$9)
  --image <path>            attach an image to the prompt (repeatable; png/jpeg/gif/webp; needs --prompt/--template)
  --provider <id>           model provider (default: settings or fake)
  --model <id>              model id (default: settings defaultModel)
  --command <name> [args]   run an extension command after ready (repeatable)
  --session-file <path>     explicit session file location
  --resume <file|id>        resume an existing session
  --fork <file|id>          fork a new session from an existing one
  --list-sessions           print the session listing as NDJSON, then exit
  --print-usage             emit a cumulative usage event before the transcript
  -p, --print               print mode: stdout = final assistant text only (same as --mode text)
  --mode <m>                text|json|rpc — text = --print; json = upstream-shaped event stream; rpc = the upstream NDJSON command protocol on stdin/stdout (no --prompt/--scenario)
  --bench                   measure the perf baseline (cold start, idle RSS, bridge throughput), then exit
  -h, --help                show this help and exit

env: PI_CODING_AGENT_DIR (agent dir, upstream name), PI_RUST_SIDECAR (sidecar entry), PI_RUST_NODE (node binary).
exit codes: 0 clean · 1 fatal (startup/config) · 2 model-script or shutdown failure · 3 degraded (sidecar crashed mid-run; built-ins kept working)
```
