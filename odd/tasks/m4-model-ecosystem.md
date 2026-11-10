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
| T1 | S1 | 401 | #21 | inline TDD | — |
| T2 | S2 | 407 | #22 | inline TDD | — |
| T3 | S3 | 406 | #23 | inline TDD | — |
| T4 | S4 | 402 | #24 | inline TDD (explorer for wire details) | — |
| T5 | S5 | 403 | #25 | inline TDD | — |
| T6 | S6 | 404 | #26 | inline TDD | — |
| T7 | S7 | 405 | #27 | inline TDD (explorer for sigv4/converse) | — |
| T8 | S8 | 408 | #28 | inline TDD | — |
| T9 | — | — | — | matrix re-run, docs, PR | — |

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
