# Milestone 2: Real model providers (OpenAI-compatible + Anthropic)

Rust core talks to real model providers behind the unchanged extension bridge.
Sidecar and bridge protocol v0 stay byte-identical; the fake scripted driver stays
for scenarios and tests.

## Specs

- S1: The Rust core runs real model conversations: the agent loop asks the provider
  for the next assistant turn (text and/or tool calls) using the conversation so far
  (milestone choice: "Echter Model-Provider").
- S2: Both provider variants are delivered in this milestone (user: "Beide parallel"):
  (a) OpenAI-compatible `chat/completions` (covers OpenAI, local models,
  solheim/qwen-style endpoints), (b) Anthropic native `messages` API.
- S3: Credentials come ONLY from environment variables (`OPENAI_API_KEY`,
  `OPENAI_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`); no key is ever read
  from config files or committed. The scripted fake driver and `--scenario` mode
  remain available and unaffected.
- S4: Original, unmodified extensions keep working: bridge protocol v0 is unchanged
  (no new wire messages); provider selection is core-side only.

## Tasks

- T1 (route: inline): `ModelDriver` becomes async `complete(conversation) -> Result<AssistantTurn>`;
  `FakeDriver` adapts; agent loop awaits; all existing tests stay green. Commit: `refactor(model): async ModelDriver::complete(conversation)`.
- T2 (route: inline): OpenAI-compatible driver: reqwest streaming (SSE),
  message/tool mapping (system/user/assistant/tool, tool_calls by index with
  argument concatenation), non-2xx error mapping. Mock-server tests via tokio
  TcpListener (no new test dep). Commit: `feat(model): OpenAI-compatible chat/completions driver`.
- T3 (route: inline): Anthropic driver: streaming SSE events (content_block_*),
  tool_use block assembly (input_json_delta accumulation), system top-level field,
  `anthropic-version` header. Mock-server tests. Commit: `feat(model): Anthropic messages driver`.
- T4 (route: inline): CLI wiring `--provider fake|openai|anthropic`, `--model <id>`,
  `--prompt <text>` (initial user message in provider mode); env-var config; README +
  docs updates; full verification. Commit: `feat(cli): provider selection, --prompt, docs`.

## Log

- L1: User request verbatim: "create an push PR. Dann nächster milestone" — milestone
  choice: "Echter Model-Provider" (Rust Core speaks Anthropic/OpenAI: real agent loops
  behind the bridge, bridge unchanged); provider choice: "Beide parallel" (both APIs in
  one milestone).
- L2: Design decisions: (a) `#[async_trait]` avoided — no new macro dep; the driver is
  used as `dyn`, so the trait returns `Pin<Box<dyn Future + Send + '_>>` (AFIT is not
  object-safe). (b) New deps: `reqwest` (rustls-tls, json, stream) + `futures-util`
  (StreamExt for the SSE byte stream) — both minimal, no other additions. (c) Provider
  tests run against a local mock HTTP server (tokio TcpListener, canned SSE), never
  against a live API; live-call example is documented but not executed in CI.
  (d) Env names follow upstream pi-ai conventions (`OPENAI_API_KEY`,
  `ANTHROPIC_API_KEY`, L29/L90 of packages/ai/src/env-api-keys.ts); base URLs default
  to `https://api.openai.com/v1` and `https://api.anthropic.com`.
- L3: PR #2 (milestone 1) open and MERGEABLE on ripmav/pi-rust (merge is the user's
  decision); base `main` = 978c3aa (scaffold), feature branch `feat/extension-bridge`
  — this milestone builds on it.
- L4: Slice boundary: milestone 1 ships as PR #2 (already split); milestone 2 is the
  next PR slice (T1-T4 + this doc) — one PR per milestone, work-unit commits inside.
