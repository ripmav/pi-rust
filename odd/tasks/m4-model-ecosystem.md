# M4 — Model Ecosystem (feature doc)

Status: in progress · Branch: `feat/m4-model-ecosystem` · Base: `main` (M3 in)

## Specs

- S1 (WP-401): OpenAI-compatible family table, verified against upstream
  `packages/ai/src/providers/*.ts` (085c84e): groq
  (`https://api.groq.com/openai/v1`), cerebras (`https://api.cerebras.ai/v1`),
  deepseek (`https://api.deepseek.com`), together (`https://api.together.ai/v1`),
  huggingface (`https://router.huggingface.co/v1`), openrouter
  (`https://openrouter.ai/api/v1`) — all `openAICompletionsApi()` upstream.
  Each id resolves to family openai + default base + key env (WP table test);
  mock-server test per family via one shared harness; custom base URL via
  env/flag already works (M2). Providers whose upstream wire API is NOT
  completions (xai = responses, minimax = anthropic, mistral = conversations,
  fireworks = custom, openai-codex = backend) get a typed
  "not supported in v0 (wire API: …)" error — recorded here, not silent.
- S2 (WP-407): token & cost accounting — every model response records usage
  (input/output/cache tokens + cost from registry prices) into the session
  (usage entries, WP-304 format); cumulative totals; `--print-usage` headless
  flag; unknown price → 0, no crash. Drivers capture real usage from the
  wire (openai: `stream_options.include_usage` final chunk; anthropic:
  message_start/message_delta).
- S3 (WP-406): runtime `setModel` — `ctx.setModel` from an extension actually
  switches the active driver mid-session (new provider/model apply to the
  next turn, key re-resolved via WP-303); invalid model id → request declined
  (success:false + reason), active model unchanged; session gets a
  `model_change` entry; `pi.model` reflects the switch.
- S4 (WP-402): Google/Gemini native driver (`generateContent` streaming,
  google + google-vertex variant): request shape (systemInstruction, tools,
  history mapping incl. tool results), SSE/stream assembly (text +
  functionCall), non-2xx mapping — mock-server tests.
- S5 (WP-403): Azure OpenAI variant: `api-version` (query/header per
  upstream), `api-key` header instead of Bearer, deployment name passes
  through as the model — mock tests.
- S6 (WP-404): Cloudflare: `cloudflare-ai-gateway` (OpenAI-compat path under
  account URL + auth token) and `cloudflare-workers-ai` (native invoke) —
  mock tests per sub-provider (URL/headers/stream per upstream files).
- S7 (WP-405): AWS Bedrock: SigV4-signed requests (manual sigv4, no SDK
  dependency), `converse`/`converse-stream` event mapping, profile/region
  from env (`AWS_PROFILE`, `AWS_REGION`, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/
  `AWS_SESSION_TOKEN`); mock tests: signature shape (deterministic vectors),
  stream assembly, auth-failure mapping.
- S8 (WP-408): `--image <path>` headless input; openai + anthropic drivers
  emit image content blocks for registry-capable models; non-capable model →
  clear error; file errors (missing/not-an-image) typed.

## Tasks

| ID | Spec | WP | Issue | Route | Commit |
|----|------|----|-------|-------|--------|
| T1 | S1 | 401 | #21 | inline TDD | cc54ae9 |
| T2 | S2 | 407 | #22 | inline TDD | 96ea486 |
| T3 | S3 | 406 | #23 | inline TDD | 3f2628e |
| T4 | S4 | 402 | #24 | inline TDD (explorer for wire details) | — |
| T5 | S5 | 403 | #25 | inline TDD | 1f20cd1 |
| T6 | S6 | 404 | #26 | inline TDD | bd62027 |
| T7 | S7 | 405 | #27 | inline TDD (explorer for sigv4/converse) | — |
| T8 | S8 | 408 | #28 | inline TDD | 41b0461 |
| T9 | — | — | — | matrix re-run, docs, PR | 9463b53 + PR |

## Log

- L1: User: "merge done. go on with the next" (PR #20/M3 merged into main
  3023292). M4 = next milestone per `roadmap/01-milestones.md`.
- L2: Compat-family recon (upstream 085c84e, `packages/ai/src/providers/`):
  v0 completions family = groq, cerebras, deepseek, together, huggingface,
  openrouter (+ openai M2). Upstream non-completions APIs: xai
  (openAIResponsesApi), minimax (anthropicMessagesApi @
  api.minimax.io/anthropic), mistral (mistralConversationsApi), fireworks
  (custom api object), openai-codex (chatgpt.com/backend-api) → S1 typed
  errors. No ollama provider file exists upstream at this pin (WP-401
  "ollama" dropped, recorded here). deepseek base has NO /v1 (upstream);
  groq base has /openai/v1.
- L3: Execution order T1→T8 as listed: 407 before the new drivers so the
  usage-capture pattern (S2) is in place for 402–405 to follow; 406 after
  401 so setModel can switch across families.
- L4: T1/WP-401 done (cc54ae9): `provider::compat_family_base` (table per
  L2) + `not_supported_v0` (typed wire-API rejections); `build_provider`
  now serves the whole completions family (per-family BASE_URL override
  env `<PROVIDER>_BASE_URL`, key via WP-303); `ProviderDriver::complete`
  forward. Tests: table, rejections, per-family mock harness on the M2
  tokio mock. Suite: 38 in-crate + 14 e2e + 42 smoke.
- L5: T2/WP-407 done (96ea486): `Usage` in model.rs (input/output/cacheRead/
  cacheWrite + cost_usd from registry Cost per 1M); openai driver captures
  usage from the final chunk (request sends `stream_options.include_usage`),
  anthropic from `message_start` (input/cache) + `message_delta` (output);
  `Session::set_usage` writes upstream-shaped usage entries; agent loop
  records per turn + cumulative Core totals; `--print-usage` emits a
  cumulative `usage` event before the final transcript. e2e: mock gpt-4o-mini
  run verifies event + session entry + exact registry cost (4.5e-5 USD).
  Suite: 39 in-crate + 15 e2e + 42 smoke.
- L6: T3/WP-406 done (3f2628e): driver selection moved per turn (live
  drivers re-read each turn; stateful fake driver stays per-run);
  set_model_request validates via bundled catalog, re-resolves credentials
  (WP-303) and only then swaps `Core.provider` (Mutex); declines leave model
  + driver + session untouched. BUGFIX found by e2e: `return decline(...)`
  inside the event_task match arm returned from the whole event_task,
  killing the sidecar->core event pump on every decline (silent second
  request drop, decision hangs); the arm now falls through. e2e
  (deterministic via the awaited `before_agent_start` decision event): run 1
  on groq, switch confirmed during run 2, run 2 on openai; exactly one
  model_change entry; declined id never recorded. Suite: 39+16+42.
  NOTE: F&F `agent_start` handlers must not be relied on for request/response
  work (core does not await them — matches upstream semantics).
- L7: T4/WP-402 done (905484a): `provider::google::GoogleDriver` (SSE
  `:streamGenerateContent?alt=sse`, `x-goog-api-key`; wire shapes mirrored
  from upstream google-generative-ai.ts/google-shared.ts: systemInstruction,
  contents with role model/user, consecutive toolResults merged into one
  user turn as functionResponse parts {output|error}, tools as
  functionDeclarations with parametersJsonSchema; stream assembly text +
  functionCall with stable ids; usageMetadata (prompt-cached, candidates+
  thoughts, cached); finishReason mapping (STOP/MAX_TOKENS/SAFETY/
  RECITATION/TOOL_USE; other/missing = error). build_provider: `google`
  (GEMINI_API_KEY or auth.json; GOOGLE_BASE_URL override) and
  `google-vertex` (GOOGLE_CLOUD_PROJECT + GOOGLE_CLOUD_LOCATION, API key
  only — ADC is a typed v0 rejection). v0 drops: thought parts/signatures,
  image tool-result parts (WP-408), thinkingConfig. Tests: mapping, tools,
  stream assembly (text+functionCall+usage), non-2xx, build variants.
  Suite: 44 in-crate + 16 e2e + 42 smoke.
- L8: Delivery strategy recorded (slice 1120 lines / 9 commits): ONE
  milestone PR (M2/M3 precedent, PR #4/#10/#20) — the work-unit commits
  keep the PR reviewable per slice. No forced mid-branch PRs.
- L9: T5/WP-403 done (1f20cd1): Azure OpenAI = OpenAiDriver
  `new_azure(deployment, api_version)`: URL `{base}/deployments/{dep}/
  chat/completions?api-version={ver}`, `api-key` header instead of bearer,
  body keeps catalog model id (upstream withDeploymentName). Base from
  AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME (->
  https://{r}.openai.azure.com/openai/v1), deployment from
  AZURE_OPENAI_DEPLOYMENT_NAME_MAP (model=dep, comma) or model id,
  AZURE_OPENAI_API_VERSION (default v1), key AZURE_OPENAI_API_KEY
  (auth.json or env). Mock test asserts deployment path, api-version,
  api-key header and absence of authorization. Suite: 45 in-crate + 16 e2e
  + 42 smoke.
- L10: T6/WP-404 done (bd62027): Cloudflare Workers AI + AI Gateway =
  OpenAiDriver at the catalog base URLs (workers-ai:
  https://api.cloudflare.com/client/v4/accounts/{acct}/ai/v1 —
  OpenAI-compatible; gateway chat models:
  https://gateway.ai.cloudflare.com/v1/{acct}/{gw}/compat — unified API,
  both verified against the pinned data files' openai-completions
  baseUrl). CLOUDFLARE_ACCOUNT_ID (+ CLOUDFLARE_GATEWAY_ID for the
  gateway) from env with typed errors; CLOUDFLARE_API_KEY bearer (
  auth.json or env). No base-URL override env (upstream has none; mock
  tests hit the driver directly). Suite: 46 in-crate + 16 e2e + 42 smoke.
- L11: T7/WP-405 done (a329fad): `provider::bedrock::BedrockDriver` —
  manual SigV4 (sha2+hmac RustCrypto, no AWS SDK per S7): canonical
  request with spec-required trailing header newline, service
  `bedrock`, session-token variant; two reference vectors computed
  independently (Python hmac/sha256) pin the implementation. Wire:
  POST /model/{id}/converse-stream with Converse messages (toolUse /
  toolResult{status}), systemInstruction via `system`, tools as
  toolSpec/inputSchema.json. AWS event-stream framing parsed by hand
  (prelude 16 = total4+len4+crc8, headers name/type/value, message CRC
  8, JSON payload; CRCs not verified in v0 — recorded as shortcut).
  Events: messageStart (role check), contentBlockStart/Delta
  (text + toolUse id/name/input-fragment accumulation), messageStop,
  metadata usage (input/output/cacheRead), exception events -> typed
  error. Env credentials only (AWS_REGION default us-east-1,
  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN);
  AWS_PROFILE -> typed v0 rejection. Mock got a byte-exact
  `start_mock_bytes` (event-stream framing breaks UTF-8 String
  transport at totals >= 128 — caught in test). Suite: 51 in-crate +
  16 e2e + 42 smoke.
- L12: T8/WP-408 done (41b0461): `--image <path>` (repeatable) on
  prompt/template runs. Magic-byte MIME detection (png/jpeg/gif/webp —
  the upstream `@file` image set), own minimal base64 encoder (no new
  deps), attachment rides QueuedUser into the prompt user message as
  pi-ai `{type:"image",mimeType,data}` blocks (session file keeps them).
  Drivers: OpenAI wire -> `image_url` data-URL parts (text part kept),
  Anthropic -> `source:{type:base64,media_type,data}` blocks. Capability
  gate: registry `input` must contain "image" (text-only models like
  openai/gpt-4 -> typed error naming the input list); fake driver and
  non-openai-wire providers (google/bedrock/vertex) -> typed v0
  rejection; --image + --scenario -> typed rejection. File errors
  typed (cannot read / not a supported format). Upstream note: original
  pi ships images via `@file` args; S8 deliberately adds the explicit
  --image flag. Tests: mime/b64/typed-error units, both driver mapping
  units, 3 e2e (data URL in the wire request + session image block,
  text-only model rejection, file errors). Suite: 56 in-crate + 19 e2e
  + 42 smoke.
- L13: T9 (M4 close): live matrix smoke across all 13 provider ids
  (typed credential/base/v0-rejection errors verified per provider;
  wire-level matrix covered by the in-crate mock tests:
  every_compat_family_reaches_mock_completions + google/azure/
  cloudflare/bedrock driver tests). README rewritten to the M4 matrix
  (wires, credential envs, auth.json precedence, v0 rejections,
  --image/--print-usage/setModel). Final suite: 56 in-crate + 19 e2e +
  42 smoke green. Branch: 17 work-unit commits (~2300 authored lines).
