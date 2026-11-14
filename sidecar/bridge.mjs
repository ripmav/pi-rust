/**
 * Bridge protocol server (docs/bridge-protocol.md v2).
 * Runs the ORIGINAL upstream loader (discoverAndLoadExtensions) and
 * ExtensionRunner; dispatches core commands, runs decision handlers, and
 * streams events on stdout (logs on stderr).
 */
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { createSessionView } from "./session.mjs";
import { createDialogManager, createBridgeActions, modelRegistryStub, log } from "./api.mjs";

const DIST =
	process.env.PI_EXTENSION_SIDECAR_DIST ??
	path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "upstream", "packages", "coding-agent", "dist", "index.js");
const ca = await import(DIST);
// The prompt builder is not re-exported by dist/index.js; import the built
// module directly (source of truth: src/core/system-prompt.ts).
const { buildSystemPrompt, normalizeBuildSystemPromptOptions } = await import(path.join(path.dirname(DIST), "core", "system-prompt.js"));
// WP-702: upstream MCP server config validation (source of truth:
// src/core/mcp-servers.ts) — not re-exported by dist/index.js either.
const { validateMcpServerConfig } = await import(path.join(path.dirname(DIST), "core", "mcp-servers.js"));
// WP-1005: the llama extension factory is not re-exported by dist/index.js.
const { default: llamaExtensionFactory } = await import(path.join(path.dirname(DIST), "extensions", "llama", "index.js"));

const F_AND_FORGET = new Set(
	"session_start agent_start agent_end turn_start turn_end message_start message_update message_end set_model set_thinking_level session_compact session_compact_failed".split(" "),
);
const message = (err) => (err instanceof Error ? err.message : String(err));
const MAX_LINE = 1024 * 1024; // v0: a single NDJSON command line never exceeds 1 MB
const response = (id, command, success, data, error) =>
	({ id, type: "response", command, success, ...(success ? (data === undefined ? {} : { data }) : { error }) });

export async function runBridge({ extensionDirs, agentDir }) {
	const cwd = process.cwd();
	// WP-702: keep the sidecar's agent-dir-derived paths (mcp.log, the OAuth
	// credential store) inside the core's agent dir; upstream getAgentDir()
	// resolves PI_CODING_AGENT_DIR first.
	if (agentDir) process.env.PI_CODING_AGENT_DIR = agentDir;
	const output = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");

	// A broken pipe (core died) must not crash the sidecar: log and exit orderly.
	process.stdout.on("error", (err) => {
		log(`stdout write failed: ${message(err)}; exiting`);
		process.exit(1);
	});

	// ---- stdin pump (upstream jsonl semantics: \n-delimited, strip \r, flush non-empty tail at end)
	const queue = [];
	let buf = "";
	let closed = false;
	let wakeFn = null;
	process.stdin.setEncoding("utf8");
	process.stdin.on("data", (chunk) => {
		buf += chunk;
		let i;
		while ((i = buf.indexOf("\n")) !== -1) {
			let line = buf.slice(0, i);
			buf = buf.slice(i + 1);
			if (line.endsWith("\r")) line = line.slice(0, -1);
			if (!line.trim()) continue;
			// v0 hardening: a single command line never exceeds 1 MB. Reject it
			// (parse-error model) instead of letting JSON.parse eat the memory.
			if (line.length > MAX_LINE) {
				output({ id: null, type: "response", command: "parse", success: false, error: `line too long (${line.length} bytes, max ${MAX_LINE})` });
				continue;
			}
			queue.push(line);
		}
		// An unterminated line already past the cap means a broken protocol: report
		// and reset the buffer instead of accumulating forever (it would swallow
		// every later line, including valid ones).
		if (buf.length > MAX_LINE) {
			output({ id: null, type: "response", command: "parse", success: false, error: `line too long (unterminated, max ${MAX_LINE} bytes)` });
			buf = "";
		}
		wake();
	});
	process.stdin.on("end", () => {
		closed = true;
		if (buf.trim()) queue.push(buf);
		buf = "";
		wake();
	});
	function wake() {
		const fn = wakeFn;
		wakeFn = null;
		fn?.();
	}
	const nextLine = () =>
		new Promise((resolve) => {
			const check = () => {
				if (queue.length) resolve(queue.shift());
				else if (closed) resolve(null);
				else wakeFn = check;
			};
			check();
		});

	// ---- state (filled by init)
	let runner = null;
	let extensions = [];
	let session = null;
	let dialog = null;
	let state = { tools: new Map(), allToolInfo: [], commands: [], activeTools: new Set(), providers: new Map(), nativeProviders: new Map() };
	let capabilities = { ui: [], timeoutMs: 30000 };
	let initialized = false;
	let shuttingDown = false;
	let exitScheduled = false;

	const pendingRequests = new Map(); // sidecar->core: id -> (response) => void
	let nestedSeq = 0;
	const sendRequest = (payload) =>
		new Promise((resolve) => {
			const id = crypto.randomUUID();
			const timer = setTimeout(() => {
				pendingRequests.delete(id);
				resolve({ id, type: "response", command: payload.type, success: false, error: "sidecar request timed out" });
			}, capabilities.timeoutMs);
			pendingRequests.set(id, (response) => {
				clearTimeout(timer);
				resolve(response);
			});
			output({ id, ...payload });
		});

	function shutdown() {
		shuttingDown = true;
		if (exitScheduled) return;
		exitScheduled = true;
		void (async () => {
			await new Promise((resolve) => process.stdout.write("", resolve));
			process.exit(0);
		})();
	}

	// Handler throws never fail the command: emit extension_error, resolve to the protocol default.
	const safe = (promise, fallback, eventName) =>
		promise.catch((err) => {
			runner.emitError({ extensionPath: "<unknown>", event: eventName, error: message(err) });
			return fallback;
		});

	// Decision command: fast path null when unsubscribed, else run the emitter and map the result.
	// WP-1003: assigned in the tools block; decisions flush pending tool-set
	// events BEFORE writing their response, so the core's drain barrier applies
	// them (order on the pipe = order of effect).
	let flushToolsEventsNow = () => {};
	const decision = async (id, type, eventName, emitFn, fallback = null, map = (r) => r ?? null) => {
		if (!runner.hasHandlers(eventName)) return response(id, type, true, null);
		const data = map(await safe(emitFn(), fallback, eventName));
		flushToolsEventsNow();
		return response(id, type, true, data);
	};

	// ---- WP-701: extension-registered providers (legacy config form) ----

	// ready/providers_changed `providers` payload: [{name, models:[{provider,id,api,name?}]}].
	const providersPayload = () => {
		const legacy = [...state.providers.entries()].map(([name, config]) => ({
			name,
			models: (config?.models ?? [])
				.filter((m) => m?.id)
				.map((m) => ({
					provider: name,
					id: m.id,
					api: m.api ?? config?.api,
					...(m.name ? { name: m.name } : {}),
				})),
		}));
		// WP-1005: native-form providers (the llama extension registers a
		// Provider object) appear in the same payload — models come from
		// getModels() (populated by the sidecar-side refreshModels when a
		// router is configured).
		const native = [...state.nativeProviders.values()].map((p) => ({
			name: p.id,
			models: (p.getModels?.() ?? [])
				.filter((m) => m?.id)
				.map((m) => ({
					provider: p.id,
					id: m.id,
					api: m.api,
					...(m.name && m.name !== m.id ? { name: m.name } : {}),
				})),
		}));
		return [...legacy, ...native];
	};

	// WP-1005: capture the native Provider OBJECT form (upstream
	// registerProvider overload / registerNativeProvider). The object stays
	// sidecar-side (functions are not JSON); only the model list crosses the
	// wire via ready/providers_changed, and the core's existing
	// provider_stream request invokes the object here.
	const captureNativeProvider = (provider) => {
		if (!provider || typeof provider.id !== "string" || typeof provider.getModels !== "function" || (typeof provider.stream !== "function" && typeof provider.streamSimple !== "function")) {
			log("registerNativeProvider: not a Provider object (id/getModels/stream required), ignored");
			return;
		}
		state.nativeProviders.set(provider.id, provider);
		emitProvidersChanged();
	};

	// Post-ready registration changes are pushed to the core (it re-keys its
	// per-turn routing + set_model validation on the payload).
	const emitProvidersChanged = () => {
		if (initialized) output({ type: "providers_changed", providers: providersPayload() });
	};

	// ProviderConfig.apiKey: literal or $ENV_VAR/${ENV_VAR} interpolation.
	// Leading `!command` and a missing env var are typed errors (v0 non-goals).
	const resolveApiKey = (raw) => {
		if (raw === undefined) return undefined;
		if (typeof raw !== "string") throw new Error("apiKey must be a string (literal or $ENV_VAR)");
		if (raw.startsWith("!")) throw new Error("apiKey !command is not supported in the v0 bridge (literal or $ENV_VAR only)");
		return raw.replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)/g, (_all, braced, plain) => {
			const name = braced ?? plain;
			const value = process.env[name];
			if (value === undefined) throw new Error(`apiKey env var ${name} is not set`);
			return value;
		});
	};

	// Terminal error event per the upstream AssistantMessageEvent contract
	// (stopReason "error" + errorMessage); wraps streamSimple setup failures.
	const providerErrorEvent = (model, err) => ({
		type: "error",
		reason: "error",
		error: {
			role: "assistant",
			content: [],
			api: model?.api,
			provider: model?.provider,
			model: model?.id,
			usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
			stopReason: "error",
			errorMessage: message(err),
			timestamp: Date.now(),
		},
	});

	// Core->sidecar `provider_stream`: invoke the captured streamSimple, ack
	// first, then forward every upstream AssistantMessageEvent as
	// {type:"provider_stream_event", streamId, event}. Terminal `done`/`error`
	// events end the stream (forwarded like any other event).
	async function handleProviderStream(command) {
		const { id } = command;
		const model = command.model;
		if (!model?.provider || !model?.id) {
			return output(response(id, "provider_stream", false, undefined, "provider_stream needs a model with provider and id"));
		}
		// WP-1005: native Provider object form (e.g. the unmodified llama
		// extension). The object never crosses the wire — the full model is
		// re-fetched sidecar-side, env-based auth is resolved here (upstream
		// AuthContext with process.env), and the provider's own stream runs.
		const nativeProvider = state.nativeProviders.get(model.provider);
		if (nativeProvider) {
			const full = (nativeProvider.getModels?.() ?? []).find((m) => m?.id === model.id);
			if (!full) {
				return output(response(id, "provider_stream", false, undefined, `native provider ${model.provider} has no model ${model.id}`));
			}
			const authCtx = { env: async (n) => process.env[n] ?? null, fileExists: async () => false };
			const hasAuth = typeof nativeProvider.auth?.apiKey?.resolve === "function";
			let auth;
			try {
				auth = hasAuth ? await nativeProvider.auth.apiKey.resolve({ ctx: authCtx, credential: undefined }) : {};
			} catch {
				auth = undefined;
			}
			if (hasAuth && !auth?.auth) {
				const streamId = command.streamId ?? crypto.randomUUID();
				output(response(id, "provider_stream", true, { ok: true, streamId }));
				forwardStream(streamId, (async function* () {
					yield providerErrorEvent(full, new Error(`provider ${model.provider} is not configured (env auth unresolved — is the required environment set?)`));
				})(), full);
				return;
			}
			const streamId = command.streamId ?? crypto.randomUUID();
			output(response(id, "provider_stream", true, { ok: true, streamId }));
			let stream;
			try {
				const fn = nativeProvider.stream ?? nativeProvider.streamSimple;
				stream = await Promise.resolve(
					fn.call(nativeProvider, full, { messages: command.messages ?? [] }, {
						...(command.options ?? {}),
						...(auth?.auth?.apiKey !== undefined ? { apiKey: auth.auth.apiKey } : {}),
						...(auth?.auth?.baseUrl !== undefined ? { baseUrl: auth.auth.baseUrl } : {}),
						...(auth?.auth?.headers ? { headers: auth.auth.headers } : {}),
						...(auth?.env ? { env: auth.env } : {}),
					}),
				);
			} catch (err) {
				forwardStream(streamId, (async function* () {
					yield providerErrorEvent(full, err);
				})(), full);
				return;
			}
			forwardStream(streamId, stream, full);
			return;
		}
		const config = state.providers.get(model.provider);
		if (!config) return output(response(id, "provider_stream", false, undefined, `unknown extension provider: ${model.provider}`));
		if (typeof config.streamSimple !== "function") {
			return output(response(id, "provider_stream", false, undefined, `extension provider ${model.provider} has no streamSimple`));
		}
		let options = { ...(command.options ?? {}) };
		try {
			options.apiKey = resolveApiKey(config.apiKey);
		} catch (err) {
			return output(response(id, "provider_stream", false, undefined, message(err)));
		}
		// Reconstruct the registered model: provider-level config fields
		// (api/baseUrl/headers) back the per-model entry the core sent.
		const registered = (config.models ?? []).find((m) => m?.id === model.id);
		const streamModel = { ...registered, ...model, provider: model.provider };
		if (!streamModel.api && config.api) streamModel.api = config.api;
		if (!streamModel.baseUrl && config.baseUrl) streamModel.baseUrl = config.baseUrl;
		if (config.headers) streamModel.headers = { ...config.headers, ...(streamModel.headers ?? {}) };
		const streamId = command.streamId ?? crypto.randomUUID();
		output(response(id, "provider_stream", true, { ok: true, streamId }));
		const emit = (event) => output({ type: "provider_stream_event", streamId, event });
		let stream;
		try {
			// TranscriptContext is {messages} (the upstream brand is
			// compile-time only); streamSimple implementations read
			// context.messages.
			stream = await Promise.resolve(config.streamSimple(streamModel, { messages: command.messages ?? [] }, options));
		} catch (err) {
			emit(providerErrorEvent(streamModel, err));
			return;
		}
		forwardStream(streamId, stream, streamModel);
	}

	// Shared tail for both provider forms: forward every event, require a
	// terminal one.
	async function forwardStream(streamId, stream, streamModel) {
		const emit = (event) => output({ type: "provider_stream_event", streamId, event });
		try {
			let terminal = false;
			for await (const event of stream) {
				emit(event);
				if (event?.type === "done" || event?.type === "error") terminal = true;
			}
			if (!terminal) emit(providerErrorEvent(streamModel, new Error("provider stream ended without a terminal event")));
		} catch (err) {
			emit(providerErrorEvent(streamModel, err));
		}
	}

	// ---- WP-702: the built-in mcp extension's `pi` surface (upstream
	// extensions/mcp/index.ts uses exactly: on, registerTool, registerCommand,
	// registerToolRenderer, getMcpServers, getAllTools, getActiveTools,
	// setActiveTools). Writes go into an Extension-shaped object; actions
	// delegate to the shared runtime (the same one user extensions use).
	function createBuiltinMcpApi(extension, runtime) {
		return {
			on: (event, handler) => {
				const list = extension.handlers.get(event) ?? [];
				const wrapped = (...args) => handler(...args);
				list.push(wrapped);
				extension.handlers.set(event, list);
				return () => {
					const handlers = extension.handlers.get(event);
					if (!handlers) return;
					const i = handlers.indexOf(wrapped);
					if (i !== -1) handlers.splice(i, 1);
					if (handlers.length === 0) extension.handlers.delete(event);
				};
			},
			registerTool: (tool) => {
				if (typeof tool.parameters !== "object" || tool.parameters === null || Array.isArray(tool.parameters)) {
					throw new Error(`Tool "${tool.name}" registered by extension "${extension.path}" must define an object parameter schema.`);
				}
				extension.tools.set(tool.name, { definition: tool, sourceInfo: extension.sourceInfo });
				runtime.refreshTools();
			},
			registerCommand: (name, options) => {
				extension.commands.set(name, { name, sourceInfo: extension.sourceInfo, ...options });
			},
			// TUI-only: accepted, ignored (bridge precedent).
			registerToolRenderer: (resolver) => {
				extension.toolRenderers ??= [];
				extension.toolRenderers.push(resolver);
			},
			getMcpServers: () => runtime.mcpServers.list(),
			getAllTools: () => runtime.getAllTools(),
			getActiveTools: () => runtime.getActiveTools(),
			setActiveTools: (names) => runtime.setActiveTools(names),
			// WP-1004: the codemode factory needs the pi surface bound by
			// bindCore (runner.ts:431/437) — it is loaded after bindCore.
			appendEntry: (entry) => runtime.appendEntry(entry),
			getSettings: () => runtime.getSettings(),
			// WP-1005: the llama extension calls pi.registerProvider(providerObject) —
			// same loader dispatch as user extensions (string = legacy config
			// form, object = native form).
			registerProvider: (name, config) => {
				if (typeof name === "string" && config && typeof config === "object") {
					state.providers.set(name, config);
					emitProvidersChanged();
				} else if (name && typeof name === "object") {
					captureNativeProvider(name);
				} else {
					log(`registerProvider on ${extension.path}: unsupported form, ignored`);
				}
			},
		};
	}

	// Host the BUILT-IN mcp extension on the shared runtime. Upstream main.ts
	// injects createMcpExtension() via extensionFactories; the dist bundle
	// does not export loadExtensionFromFactory, so the factory runs against
	// the minimal API above. The original loader stays untouched for user
	// extensions. `loadConfig` reads no files: the core owns mcp.json (702c)
	// and every server arrives through the registerMcpServer path.
	function createBuiltinExtension(name) {
		return {
			path: `builtin:${name}`,
			resolvedPath: `builtin:${name}`,
			// Upstream builtin:<name> path convention (core/source-info.ts).
			sourceInfo: ca.createSyntheticSourceInfo(`builtin:${name}`, { source: "builtin" }),
			handlers: new Map(),
			tools: new Map(),
			messageRenderers: new Map(),
			entryRenderers: new Map(),
			commands: new Map(),
			flags: new Map(),
			shortcuts: new Map(),
		};
	}

	async function loadBuiltinMcpExtension(runtime) {
		const extension = createBuiltinExtension("mcp");
		const factory = ca.createMcpExtension({
			loadConfig: () => ({ servers: [], errors: [], autoEnableCodemode: false }),
		});
		await factory(createBuiltinMcpApi(extension, runtime));
		return extension;
	}

	// WP-1003: host the BUILT-IN tool-search extension the same way (upstream
	// main.ts auto-loads it; the dist bundle exports createToolSearchExtension).
	// Its tool registers INACTIVE (exposure model-only, defaultActive false) —
	// activation happens through setActiveTools, which is now observable on the
	// wire (tools_activated, protocol v5). The ranker sources tools/active set
	// from the runtime (createBuiltinMcpApi proxies to runtime.*).
	async function loadBuiltinToolSearchExtension(runtime) {
		const extension = createBuiltinExtension("tool-search");
		const factory = ca.createToolSearchExtension();
		await factory(createBuiltinMcpApi(extension, runtime));
		return extension;
	}

	// WP-1004: host the BUILT-IN codemode extension (upstream main.ts auto-loads
	// it with the models namespace). pi-rust hosts it with models: false —
	// the models.* namespace needs model-API calls the v0 bridge cannot serve
	// (a new wire event; follow-up). It registers INACTIVE (model-only,
	// defaultActive false) like tool_search; activation goes through the
	// v5 active-set path. The factory needs runtime.appendEntry/getSettings,
	// which bindCore wires, so it loads AFTER bindCore.
	async function loadBuiltinCodemodeExtension(runtime) {
		const extension = createBuiltinExtension("codemode");
		const factory = ca.createCodemodeExtension({ models: false });
		await factory(createBuiltinMcpApi(extension, runtime));
		return extension;
	}

	// WP-1005: host the BUILT-IN llama extension UNMODIFIED (upstream main.ts
	// auto-loads it; the dist factory is imported directly). Its
	// pi.registerProvider(providerObject) lands in captureNativeProvider via
	// the builtin factory API. The /llama TUI command registers but is
	// out of scope (ui.custom, WP-1006).
	async function loadBuiltinLlamaExtension(runtime) {
		const extension = createBuiltinExtension("llama");
		await llamaExtensionFactory(createBuiltinMcpApi(extension, runtime));
		return extension;
	}

	// ---- init: load extensions with the original loader, bind the runner
	async function handleInit(command) {
		if (command.protocolVersion !== 5) {
			output(response(command.id, "init", false, undefined, `Unsupported protocolVersion: ${command.protocolVersion} (sidecar speaks 5)`));
			await new Promise((resolve) => process.stdout.write("", resolve));
			process.exit(1);
		}
		capabilities = { ui: command.capabilities?.ui ?? [], timeoutMs: command.capabilities?.timeoutMs ?? 30000 };
		log(`loading extensions (dirs: ${JSON.stringify(extensionDirs)}, cwd: ${cwd})`);

		const loaded = await ca.discoverAndLoadExtensions(extensionDirs, cwd, agentDir, ca.createEventBus());
		extensions = loaded.extensions;
		loaded.errors.forEach((e) => log(`extension load error ${e.path}: ${e.error}`));
		session = createSessionView(cwd);
		runner = new ca.ExtensionRunner(extensions, loaded.runtime, cwd, session.sessionManager, modelRegistryStub);

		// WP-702: the built-in mcp extension (after the runner, before ready):
		// its handlers subscribe to session_start / mcp_servers_change and it
		// connects the servers once the core fires session_start.
		extensions.push(await loadBuiltinMcpExtension(loaded.runtime));
		extensions.push(await loadBuiltinToolSearchExtension(loaded.runtime));
		extensions.push(await loadBuiltinLlamaExtension(loaded.runtime));

		const contextActions = {
			getModel: () => session.view.model,
			getScopedModels: () => [],
			isIdle: () => true,
			isProjectTrusted: () => true,
			getSignal: () => undefined,
			abort: () => {},
			hasPendingMessages: () => false,
			shutdown,
			getContextUsage: () => undefined,
			compact: () => {}, // compaction is out of scope for v0
			getSystemPrompt: () => session.view.systemPrompt,
			getSystemPromptOptions: () => ({ cwd, ...(session.view.systemPrompt ? { forceSystemPrompt: session.view.systemPrompt } : {}) }),
			executeTool: async (callerId, name, args) => {
				const callId = `${callerId}/${++nestedSeq}`;
				const res = await sendRequest({ type: "tool_call_request", toolCallId: callId, toolName: name, input: args ?? {} });
				// Wire carries AgentToolResult; the runner expects AgentToolCallOutcome.
				const result = res.success ? res.data : { content: [{ type: "text", text: `Nested tool call failed: ${res.error}` }], details: {}, isError: true };
				return { toolCall: { type: "toolCall", id: callId, name, arguments: args ?? {} }, result, isError: Boolean(result.isError) };
			},
			getCallableTools: () => [...state.tools.values()],
		};
		const providerActions = {
			// WP-701: legacy config form only — capture the config object at
			// registration (the sidecar hosts the provider callback); last
			// registration wins (Map, not Set).
			registerProvider: (name, config) => {
				if (typeof name === "string" && config && typeof config === "object") {
					state.providers.set(name, config);
					emitProvidersChanged();
				} else {
					log("registerProvider native Provider object form is not supported in the v0 bridge (legacy config form only)");
				}
			},
			// WP-1005: native Provider object form — captured sidecar-side
			// (the llama extension registers it unmodified).
			registerNativeProvider: (provider) => captureNativeProvider(provider),
			unregisterProvider: (name) => {
				if (state.providers.delete(name)) emitProvidersChanged();
			},
			registerVirtualModel: () => log("registerVirtualModel accepted but ignored in v0 bridge"),
			unregisterVirtualModel: () => {},
		};
		runner.bindCore(createBridgeActions({ output, sendRequest, session, state, onActiveToolsChanged: (names) => applyActiveTools(names) }), contextActions, providerActions);
		runner.bindCommandContext();
		// WP-1004: after bindCore (the factory needs runtime.appendEntry/getSettings).
		extensions.push(await loadBuiltinCodemodeExtension(loaded.runtime));
		// WP-1005: populate native-provider catalogs before `ready` — the llama
		// extension's getModels() is empty until refreshModels asks the
		// configured router (env-only auth: LLAMA_BASE_URL / LLAMA_API_KEY,
		// upstream semantics). No URL → refreshModels returns early (no
		// catalog, and stream() later fails with a typed not-configured error).
		for (const provider of state.nativeProviders.values()) {
			if (typeof provider.refreshModels !== "function") continue;
			try {
				await provider.refreshModels({
					stored: { models: [] },
					credential: {
						type: "api_key",
						key: process.env.LLAMA_API_KEY || undefined,
						env: process.env.LLAMA_BASE_URL ? { LLAMA_BASE_URL: process.env.LLAMA_BASE_URL } : {},
					},
					allowNetwork: true,
					signal: AbortSignal.timeout(5000),
					// The publish gate: v0 has no model persistence, but the update
					// callback is what mutates the provider's model list.
					publish: async (args) => {
						args?.update?.();
						return true;
					},
				});
			} catch (err) {
				log(`native provider ${provider.id} refreshModels failed: ${message(err)}`);
			}
		}

		// ---- WP-702: dynamic extension tool set (bridge protocol v2) ----
		// MCP tools register AFTER ready (on connection, lazy reconnect,
		// unregister). Upstream registerTool calls runtime.refreshTools(); the
		// v0 no-op is replaced with a rebuild + `tools_changed` push so the
		// core's ext_tools stay current (ready.tools remains the initial
		// snapshot; the event is F&F and emitted only post-ready).
		const toToolInfo = (t) => ({
			name: t.name,
			label: t.label,
			description: t.description,
			...(t.promptSnippet ? { promptSnippet: t.promptSnippet } : {}),
			parameters: t.parameters,
			// WP-1003: the core filters declarations by the active set (v5);
			// exposure/defaultActive document the registration-time rule.
			...(t.exposure ? { exposure: t.exposure } : {}),
			...(t.defaultActive === false ? { defaultActive: false } : {}),
		});
		// Upstream exposure semantics (types.ts): direct/model-only activate on
		// registration unless defaultActive is false; codemode/deferred do not;
		// hidden never activates.
		const isInitiallyActive = (t) => {
			if (t.defaultActive === false) return false;
			const exposure = t.exposure ?? "direct";
			return exposure === "direct" || exposure === "model-only";
		};
		// Coalesce: MCP connections register several tools back to back; the
		// state rebuild is immediate (dispatch must see them) but the wire
		// events are one pair per tick, carrying the final set (the core
		// replaces ext_tools then applies the active set, so intermediate
		// states are unnecessary).
		// The last state actually on the wire (ready snapshot at init, then the
		// emitted pairs): emitToolsEvents is change-only, so the timer flush and
		// the synchronous decision flush never double-send the same pair.
		let flushedActive = "";
		let flushedToolsSig = "";
		const emitToolsEvents = () => {
			const active = [...state.activeTools].sort().join("\u0000");
			const sig = state.allToolInfo.map((t) => t.name).sort().join("\u0000");
			if (active === flushedActive && sig === flushedToolsSig) return;
			flushedActive = active;
			flushedToolsSig = sig;
			output({ type: "tools_changed", tools: state.allToolInfo.map(toToolInfo) });
			output({ type: "tools_activated", active: [...state.activeTools] });
		};
		let toolsEventsScheduled = false;
		const scheduleToolsEvents = () => {
			if (!initialized || toolsEventsScheduled) return;
			toolsEventsScheduled = true;
			setImmediate(() => {
				toolsEventsScheduled = false;
				emitToolsEvents();
			});
		};
		flushToolsEventsNow = () => {
			if (initialized) emitToolsEvents();
		};
		// WP-1003: the single source of truth for active-set changes (user
		// extensions via createBridgeActions.onActiveToolsChanged, built-in
		// extensions via runtime.setActiveTools): unknown and hidden tools are
		// ignored (upstream), then the core is told on the wire.
		const applyActiveTools = (names) => {
			const byName = new Map(state.allToolInfo.map((t) => [t.name, t]));
			const next = new Set();
			for (const name of names ?? []) {
				const def = byName.get(name);
				if (def && def.exposure !== "hidden") next.add(name);
			}
			state.activeTools = next;
			scheduleToolsEvents();
		};
		const rebuildTools = () => {
			const all = runner.getAllRegisteredTools();
			state.tools = new Map(ca.wrapRegisteredTools(all, runner).map((tool) => [tool.name, tool]));
			state.allToolInfo = all.map((t) => t.definition);
			// Already-active tools stay active (reconnect, unregister/re-register);
			// newly registered tools follow the initial activation rule.
			const nextActive = new Set();
			for (const t of state.allToolInfo) {
				if (t.exposure !== "hidden" && state.activeTools.has(t.name)) nextActive.add(t.name);
				else if (isInitiallyActive(t)) nextActive.add(t.name);
			}
			state.activeTools = nextActive;
			scheduleToolsEvents();
		};
		loaded.runtime.refreshTools = rebuildTools;
		loaded.runtime.getAllTools = () => state.allToolInfo;
		loaded.runtime.getActiveTools = () => [...state.activeTools];
		loaded.runtime.setActiveTools = (names) => applyActiveTools(names);

		// ---- WP-702 (702c): config-declared MCP servers ---- The core reads
		// <agent-dir>/mcp.json (upstream shape) and passes it as `mcpConfig`;
		// each entry goes through the same registerMcpServer path extensions
		// use (validation + registry + mcp_servers_change listener). The mcp
		// extension reads the registry on session_start. Upstream rule: an
		// mcp.json entry takes precedence over a same-name extension
		// registration (the registry is last-write-wins per name).
		if (command.mcpConfig !== undefined) {
			const mcpServers = command.mcpConfig.mcpServers;
			if (mcpServers !== undefined && (typeof mcpServers !== "object" || mcpServers === null || Array.isArray(mcpServers))) {
				return output(response(command.id, "init", false, undefined, "mcpConfig.mcpServers must be an object"));
			}
			const namespaces = new Map();
			for (const [name, config] of Object.entries(mcpServers ?? {})) {
				const validated = validateMcpServerConfig(name, config);
				if (typeof validated === "string") {
					return output(response(command.id, "init", false, undefined, `mcpConfig server "${name}": ${validated}`));
				}
				// Upstream namespace rule: names that differ only in `-` and `_`
				// share a namespace and must not coexist.
				const ns = `mcp__${name.replace(/-/g, "_")}`;
				if (namespaces.has(ns)) {
					return output(response(command.id, "init", false, undefined, `mcpConfig server "${name}" conflicts with "${namespaces.get(ns)}"`));
				}
				namespaces.set(ns, name);
				loaded.runtime.mcpServers.register({ name, config: structuredClone(validated), extensionPath: "mcp.json" });
			}
		}
		// WP-505: dialog wiring only for clients that advertise UI methods. Headless
		// clients (no ui methods) keep the runner's noOp UI context, so hasUI() is
		// false and extensions take their upstream no-UI defaults (e.g.
		// permission-gate blocks dangerous commands without a dialog). Initialize the
		// shared theme for the noOp context: upstream only does this in TUI startup,
		// but headless-safe extensions (plan-mode) call theme helpers inside commands,
		// and the M1 headless-safe contract keeps them working.
		if (capabilities.ui.length > 0) {
			dialog = createDialogManager({ output, uiMethods: capabilities.ui, defaultTimeoutMs: capabilities.timeoutMs, sendRequest });
			runner.setUIContext(dialog.ui, "rpc");
		} else {
			ca.initTheme();
		}
		runner.onError((err) => output({ type: "extension_error", extensionPath: err.extensionPath, event: err.event, error: err.error }));

		// extension.tools -> wrapper.ts -> tool-definition-wrapper.ts (keeps TypeBox schemas).
		// Pre-ready rebuild: no tools_changed (ready.tools is the initial snapshot).
		rebuildTools();
		state.commands = runner.getRegisteredCommands();
		// WP-705: resolve the extension shortcuts against the core's resolved
		// keybindings — upstream precedence for free: a reserved built-in
		// blocks the shortcut (diagnostic), a non-reserved built-in is
		// shadowed (extension wins, diagnostic), two extensions on one key ->
		// the later registration wins.
		state.shortcuts = runner.getShortcuts(command.keybindings ?? {});
		state.shortcutDiagnostics = runner.getShortcutDiagnostics();

		// TUI-only registrations: accepted by the loader, ignored by the bridge (log once per extension).
		for (const ext of extensions) {
			const ignored = [];
			if (ext.messageRenderers.size || ext.entryRenderers?.size || ext.toolRenderers?.length || ext.markdownTransformer)
				ignored.push("renderers");
			if (ignored.length) log(`ignoring ${ignored.join(", ")} from ${ext.path} (TUI-only in v0 bridge)`);
		}

		initialized = true;
		output(response(command.id, "init", true, { apiVersion: 0 }));
		flushedActive = [...state.activeTools].sort().join("\u0000");
		flushedToolsSig = state.allToolInfo.map((t) => t.name).sort().join("\u0000");
		output({
			type: "ready",
			apiVersion: 0,
			extensions: extensions.map((e) => ({ path: e.path, ...(e.hidden ? { hidden: true } : {}) })),
			errors: loaded.errors,
			tools: state.allToolInfo.map((t) => ({
				name: t.name,
				label: t.label,
				description: t.description,
				...(t.promptSnippet ? { promptSnippet: t.promptSnippet } : {}),
				parameters: t.parameters,
				...(t.exposure ? { exposure: t.exposure } : {}),
				...(t.defaultActive === false ? { defaultActive: false } : {}),
			})),
			activeTools: [...state.activeTools],
			commands: state.commands.map((c) => ({ name: c.name, ...(c.description ? { description: c.description } : {}) })),
			shortcuts: [...state.shortcuts.values()].map((sc) => ({
				key: sc.shortcut,
				...(sc.description ? { description: sc.description } : {}),
				extensionPath: sc.extensionPath,
			})),
			shortcutDiagnostics: state.shortcutDiagnostics.map((d) => d.message),
			flags: [...runner.getFlags().values()],
			providers: providersPayload(),
			events: [...new Set(extensions.flatMap((e) => [...e.handlers.keys()]))].sort(),
		});
		log(`ready: ${extensions.length} extensions, ${state.tools.size} tools, ${state.commands.length} commands`);
	}

	async function handleCommand(command) {
		const { type, id } = command;
		if (type === "init") return handleInit(command);
		if (!initialized) {
			if (id !== undefined) return response(id, type ?? "unknown", false, undefined, "Sidecar not initialized; send init first");
			return;
		}

		switch (type) {
			case "session_shutdown": {
				// Flag synchronously BEFORE the (async) extension emit: any
				// command queued after this one — including behind the emit —
				// must not be dispatched, regardless of runner timing.
				shuttingDown = true;
				// emitSessionShutdownEvent() (runner.ts:265) semantics: emit only when subscribed.
				const event = command.event ?? { type: "session_shutdown", reason: "quit" };
				if (runner.hasHandlers("session_shutdown")) await safe(runner.emit(event), undefined, "session_shutdown");
				shutdown();
				return;
			}
			case "extension_tool_call": {
				const tool = state.tools.get(command.toolName);
				if (!tool) return response(id, type, false, undefined, `Unknown extension tool: ${command.toolName}`);
				const result = await tool.execute(command.toolCallId, command.input ?? {}, undefined, undefined);
				// WP-1003 fix: the tool's execute may have changed the active
				// set (setActiveTools); flush before the response so the core's
				// drain barrier applies it (decisions flush the same way — the
				// setImmediate alone races the next turn under load).
				flushToolsEventsNow();
				return response(id, type, true, result);
			}
			// Returns the final input so the core executes the post-mutation arguments (in-place mutation, no re-validation).
			case "tool_call":
				return decision(id, type, "tool_call", () => runner.emitToolCall(command.event), null, (result) => ({ result: result ?? null, input: command.event.input }));
			case "tool_result":
				return decision(id, type, "tool_result", () => runner.emitToolResult(command.event));
			case "context":
				// Two-phase emitContext contract: the sidecar transforms nothing here; all handler
				// phases run once on context_with_system (the core re-attaches its system prompt in between).
				return response(id, type, true, null);
			case "context_with_system": {
				// emitContext runs both phases once; fast path only when neither has a handler.
				if (!runner.hasHandlers("context") && !runner.hasHandlers("context_with_system")) return response(id, type, true, null);
				return response(id, type, true, await safe(runner.emitContext(command.event?.messages ?? []), command.event?.messages ?? [], "context"));
			}
			case "input":
				return decision(id, type, "input", () => {
					const { text, images, source, streamingBehavior } = command.event;
					return runner.emitInput(text, images, source, streamingBehavior);
				}, { action: "continue" });
			case "before_agent_start": {
				if (!runner.hasHandlers("before_agent_start")) return response(id, type, true, null);
				const base = command.systemPrompt;
				session.view.systemPrompt = base ?? session.view.systemPrompt;
				// Merge the core's BuildSystemPromptOptions subset (sections, selectedTools,
				// skills, ...); cwd and forceSystemPrompt come from the sidecar/base.
				const options = {
					cwd,
					...(command.systemPromptOptions ?? {}),
					...(base !== undefined ? { forceSystemPrompt: base } : {}),
				};
				const combined = await safe(
					runner.emitBeforeAgentStart(command.event, options),
					{ messages: [], systemPromptOptions: options },
					"before_agent_start",
				);
				const opts = combined.systemPromptOptions;
				// Normalized base (the runner normalizes internally and applies defaults,
				// e.g. selectedTools): mutation checks must compare against the same
				// normalized shape, not the raw request options.
				const baseOpts = normalizeBuildSystemPromptOptions(options);
				// A structural mutation (sections/selectedTools/skills) renders through
				// the upstream prompt builder with the base stand-in forceSystemPrompt
				// suppressed, so the sections appear in the prompt.
				const structural =
					JSON.stringify(opts.sections ?? {}) !== JSON.stringify(baseOpts.sections ?? {}) ||
					JSON.stringify(opts.selectedTools ?? []) !== JSON.stringify(baseOpts.selectedTools ?? []) ||
					JSON.stringify(opts.skills ?? []) !== JSON.stringify(baseOpts.skills ?? []);
				// Precedence follows upstream: a forced prompt is opaque (claude-rules
				// style direct string mutations win); otherwise structural mutations
				// render, and with no mutation the final string is the (forced) base.
				const finalPrompt =
					opts.forceSystemPrompt !== options.forceSystemPrompt
						? opts.forceSystemPrompt
						: structural
							? buildSystemPrompt({ ...opts, forceSystemPrompt: undefined })
							: opts.forceSystemPrompt;
				// Return the rendered systemPrompt string only when a handler changed it; otherwise omission.
				return response(id, type, true, {
					...(combined.messages.length ? { messages: combined.messages } : {}),
					...(finalPrompt !== undefined && finalPrompt !== base ? { systemPrompt: finalPrompt } : {}),
				});
			}
			case "session_before_compact": {
				// WP-704: decision — the handler may cancel or supply the
				// compaction itself (upstream SessionBeforeCompactResult); the
				// runner's emit returns it exactly as upstream agent-session
				// consumes it.
				if (!runner.hasHandlers("session_before_compact")) return response(id, type, true, null);
				const result = await safe(runner.emit(command.event), undefined, "session_before_compact");
				if (!result) return response(id, type, true, null);
				return response(id, type, true, {
					...(result.cancel ? { cancel: true } : {}),
					...(result.compaction ? { compaction: result.compaction } : {}),
				});
			}
			case "shortcut": {
				// WP-705: the TUI dispatched a keypress to a resolved extension
				// shortcut (upstream checks these before built-ins; a match
				// consumes the key). The handler runs with the same bridge
				// context stand-ins as `command`.
				const sc = state.shortcuts.get((command.key ?? "").toLowerCase());
				if (!sc) return response(id, type, false, undefined, `No extension shortcut for key: ${command.key}`);
				const result = await sc.handler(runner.createCommandContext());
				return response(id, type, true, result ?? null);
			}
						case "user_bash":
				return decision(id, type, "user_bash", () => runner.emitUserBash(command.event));
			case "command": {
				// Bridge v0.1 addition (not in the protocol doc yet): invoke a registered extension command.
				const resolved = runner.getCommand(command.name) ?? runner.getRegisteredCommands().find((c) => c.name === command.name);
				if (!resolved) return response(id, type, false, undefined, `Unknown command: ${command.name}`);
				const result = await resolved.handler(command.args ?? "", runner.createCommandContext());
				return response(id, type, true, result ?? null);
			}
			case "provider_stream": {
				// Ack + events are emitted inside; no dispatcher response line.
				await handleProviderStream(command);
				return;
			}
			default: {
				if (F_AND_FORGET.has(type)) {
					await safe(runner.emit(command.event), undefined, type);
					if (type === "set_model" && command.event?.model) {
						session.view.model = command.event.model;
						session.sessionManager.appendModelChange(command.event.model.provider, command.event.model.id);
					}
					if (type === "set_thinking_level" && command.event?.level !== undefined)
						session.view.thinkingLevel = command.event.level;
					return; // F&F: no response line
				}
				if (id === undefined) {
					log(`unknown or unsubscribed event "${type}" ignored`);
					return;
				}
				return response(id, type, false, undefined, `Unknown command: ${type}`);
			}
		}
	}

	// ---- main loop
	for (;;) {
		const line = await nextLine();
		if (line === null) {
			if (!initialized) {
				log("stdin closed before init");
				process.exit(1);
			}
			log("stdin closed; shutting down");
			shutdown();
			return;
		}
		if (shuttingDown) break; // shutdown in flight: stop dispatching queued commands
		let parsed;
		try {
			parsed = JSON.parse(line);
		} catch (err) {
			output({ id: null, type: "response", command: "parse", success: false, error: `Failed to parse command: ${message(err)}` });
			continue;
		}
		if (parsed?.type === "response") {
			const waiter = pendingRequests.get(parsed.id);
			if (waiter) {
				pendingRequests.delete(parsed.id);
				waiter(parsed);
			} else log(`response for unknown or timed-out request ${parsed.id} ignored`);
			continue;
		}
		if (parsed?.type === "extension_ui_response") {
			// Never await here: a blocking command may be waiting for exactly this line
			// (bidirectional liveness, no head-of-line blocking in either direction).
			dialog?.respond(parsed.id, parsed);
			continue;
		}
		// Fire-and-forget dispatch (upstream rpc-mode pattern): a command may block on a
		// dialog or a sidecar->core request; later lines must still be readable.
		void (async () => {
			try {
				const resp = await handleCommand(parsed);
				if (resp) output(resp);
			} catch (err) {
				log(`command ${parsed?.type} failed: ${message(err)}`);
				output(response(parsed?.id ?? null, parsed?.type ?? "unknown", false, undefined, message(err)));
			}
		})();
	}
}
