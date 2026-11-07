/**
 * Bridge protocol server (docs/bridge-protocol.md v0).
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

const F_AND_FORGET = new Set(
	"session_start agent_start agent_end turn_start turn_end message_start message_update message_end set_model set_thinking_level".split(" "),
);
const message = (err) => (err instanceof Error ? err.message : String(err));
const response = (id, command, success, data, error) =>
	({ id, type: "response", command, success, ...(success ? (data === undefined ? {} : { data }) : { error }) });

export async function runBridge({ extensionDirs, agentDir }) {
	const cwd = process.cwd();
	const output = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");

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
			if (line.trim()) queue.push(line);
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
	let state = { tools: new Map(), allToolInfo: [], commands: [], activeTools: new Set(), providers: new Set() };
	let capabilities = { ui: [], timeoutMs: 30000 };
	let initialized = false;
	let shuttingDown = false;

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
		if (shuttingDown) return;
		shuttingDown = true;
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
	const decision = async (id, type, eventName, emitFn, fallback = null, map = (r) => r ?? null) => {
		if (!runner.hasHandlers(eventName)) return response(id, type, true, null);
		return response(id, type, true, map(await safe(emitFn(), fallback, eventName)));
	};

	// ---- init: load extensions with the original loader, bind the runner
	async function handleInit(command) {
		if (command.protocolVersion !== 0) {
			output(response(command.id, "init", false, undefined, `Unsupported protocolVersion: ${command.protocolVersion} (sidecar speaks 0)`));
			await new Promise((resolve) => process.stdout.write("", resolve));
			process.exit(1);
		}
		capabilities = { ui: command.capabilities?.ui ?? [], timeoutMs: command.capabilities?.timeoutMs ?? 30000 };
		log(`loading extensions (dirs: ${JSON.stringify(extensionDirs)}, cwd: ${cwd})`);

		const loaded = await ca.discoverAndLoadExtensions(extensionDirs, cwd, agentDir, ca.createEventBus());
		extensions = loaded.extensions;
		loaded.errors.forEach((e) => log(`extension load error ${e.path}: ${e.error}`));
		session = createSessionView(cwd);
		dialog = createDialogManager({ output, uiMethods: capabilities.ui, defaultTimeoutMs: capabilities.timeoutMs });
		runner = new ca.ExtensionRunner(extensions, loaded.runtime, cwd, session.sessionManager, modelRegistryStub);

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
			registerProvider: (name) => state.providers.add(name),
			registerNativeProvider: (provider) => state.providers.add(provider.id),
			unregisterProvider: (name) => state.providers.delete(name),
			registerVirtualModel: () => log("registerVirtualModel accepted but ignored in v0 bridge"),
			unregisterVirtualModel: () => {},
		};
		runner.bindCore(createBridgeActions({ output, sendRequest, session, state }), contextActions, providerActions);
		runner.bindCommandContext();
		runner.setUIContext(dialog.ui, "rpc");
		runner.onError((err) => output({ type: "extension_error", extensionPath: err.extensionPath, event: err.event, error: err.error }));

		// extension.tools -> wrapper.ts -> tool-definition-wrapper.ts (keeps TypeBox schemas)
		const wrapped = ca.wrapRegisteredTools(runner.getAllRegisteredTools(), runner);
		state.tools = new Map(wrapped.map((tool) => [tool.name, tool]));
		state.allToolInfo = runner.getAllRegisteredTools().map((t) => t.definition);
		state.activeTools = new Set(state.allToolInfo.map((t) => t.name));
		state.commands = runner.getRegisteredCommands();

		// TUI-only registrations: accepted by the loader, ignored by the bridge (log once per extension).
		for (const ext of extensions) {
			const ignored = [];
			if (ext.shortcuts.size) ignored.push("registerShortcut");
			if (ext.messageRenderers.size || ext.entryRenderers?.size || ext.toolRenderers?.length || ext.markdownTransformer)
				ignored.push("renderers");
			if (ignored.length) log(`ignoring ${ignored.join(", ")} from ${ext.path} (TUI-only in v0 bridge)`);
		}

		initialized = true;
		output(response(command.id, "init", true, { apiVersion: 0 }));
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
			})),
			commands: state.commands.map((c) => ({ name: c.name, ...(c.description ? { description: c.description } : {}) })),
			flags: [...runner.getFlags().values()],
			providers: [...state.providers],
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
				const options = { cwd, ...(base !== undefined ? { forceSystemPrompt: base } : {}) };
				const combined = await safe(
					runner.emitBeforeAgentStart(command.event, options),
					{ messages: [], systemPromptOptions: options },
					"before_agent_start",
				);
				const finalPrompt = combined.systemPromptOptions.forceSystemPrompt;
				// Return the rendered systemPrompt string only when a handler changed it; otherwise omission.
				return response(id, type, true, {
					...(combined.messages.length ? { messages: combined.messages } : {}),
					...(finalPrompt !== undefined && finalPrompt !== base ? { systemPrompt: finalPrompt } : {}),
				});
			}
			case "user_bash":
				return decision(id, type, "user_bash", () => runner.emitUserBash(command.event));
			case "command": {
				// Bridge v0.1 addition (not in the protocol doc yet): invoke a registered extension command.
				const resolved = runner.getCommand(command.name) ?? runner.getRegisteredCommands().find((c) => c.name === command.name);
				if (!resolved) return response(id, type, false, undefined, `Unknown command: ${command.name}`);
				await resolved.handler(command.args ?? "", runner.createCommandContext());
				return response(id, type, true);
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
