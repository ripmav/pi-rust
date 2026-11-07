/**
 * Headless ExtensionAPI wiring: the dialog subset (upstream rpc-mode
 * createDialogPromise / createExtensionUIContext semantics, blocking on
 * extension_ui_response with the negotiated timeout) and the bridge-backed
 * runtime actions. TUI-only registrations are accepted-but-ignored no-ops.
 */
import crypto from "node:crypto";

export const log = (msg) => console.error(`[sidecar] ${msg}`);

/** ModelRegistry stand-in: the v0 core never invokes providers; registration is reported only. */
export const modelRegistryStub = {
	getAll: () => [],
	getAvailable: () => [],
	find: () => undefined,
	findOfType: () => undefined,
	hasConfiguredAuth: () => false,
	getError: () => undefined,
	refresh: async () => ({}),
	registerProvider: () => {},
	registerNativeProvider: () => {},
	unregisterProvider: () => {},
	registerVirtualModel: () => {},
	unregisterVirtualModel: () => {},
};

export function createDialogManager({ output, uiMethods, defaultTimeoutMs }) {
	const supported = new Set(uiMethods ?? []);
	const pending = new Map();

	function dialog(defaultValue, request, parse) {
		// Method the core does not implement: resolve to the upstream default without a request.
		if (!supported.has(request.method)) return Promise.resolve(defaultValue);
		const id = crypto.randomUUID();
		return new Promise((resolve) => {
			const cleanup = () => {
				clearTimeout(timer);
				pending.delete(id);
			};
			// Upstream defaults on timeout: select/input/editor -> undefined, confirm -> false.
			const timer = setTimeout(() => {
				cleanup();
				resolve(defaultValue);
			}, request.timeout ?? defaultTimeoutMs);
			pending.set(id, (response) => {
				cleanup();
				resolve(parse(response));
			});
			output({ type: "extension_ui_request", id, ...request });
		});
	}

	const ui = {
		select: (title, options, opts) =>
			dialog(undefined, { method: "select", title, options, timeout: opts?.timeout }, (r) => (r.cancelled ? undefined : r.value)),
		confirm: (title, message, opts) =>
			dialog(false, { method: "confirm", title, message, timeout: opts?.timeout }, (r) => (r.cancelled ? false : r.confirmed)),
		input: (title, placeholder, opts) =>
			dialog(undefined, { method: "input", title, placeholder, timeout: opts?.timeout }, (r) => (r.cancelled ? undefined : r.value)),
		// Upstream rpc-mode has no timeout for editor; the negotiated default keeps dialogs live.
		editor: (title, prefill) =>
			dialog(undefined, { method: "editor", title, prefill, timeout: defaultTimeoutMs }, (r) => (r.cancelled ? undefined : r.value)),
		// Fire-and-forget UI channel.
		notify: (message, type) => output({ type: "extension_ui_request", id: crypto.randomUUID(), method: "notify", message, notifyType: type }),
		setStatus: (key, text) => output({ type: "extension_ui_request", id: crypto.randomUUID(), method: "setStatus", statusKey: key, statusText: text }),
		setTitle: (title) => output({ type: "extension_ui_request", id: crypto.randomUUID(), method: "setTitle", title }),
		setEditorText: (text) => output({ type: "extension_ui_request", id: crypto.randomUUID(), method: "set_editor_text", text }),
		pasteToEditor: (text) => ui.setEditorText(text),
		getEditorText: () => "",
		onTerminalInput: () => () => {},
		get theme() {
			// No TUI in bridge mode: style helpers are identity pass-throughs
			// (extensions like plan-mode call theme.fg / theme.strikethrough).
			return { fg: (_style, text) => text, strikethrough: (text) => text };
		},
		getAllThemes: () => [],
		getTheme: () => undefined,
		setTheme: () => ({ success: false, error: "Theme switching not supported in bridge mode" }),
		getToolsExpanded: () => false,
		// TUI-only surface: accepted, ignored (rpc-mode precedent).
		setWorkingMessage: () => {},
		setWorkingVisible: () => {},
		setWorkingIndicator: () => {},
		setHiddenThinkingLabel: () => {},
		setWidget: () => {},
		setFooter: () => {},
		setHeader: () => {},
		custom: async () => undefined,
		addAutocompleteProvider: () => {},
		setEditorComponent: () => {},
		getEditorComponent: () => undefined,
		setToolsExpanded: () => {},
	};

	return {
		ui,
		respond(id, response) {
			const waiter = pending.get(id);
			if (waiter) waiter(response);
			else log(`extension_ui_response for unknown or timed-out dialog ${id} ignored`);
		},
	};
}

/** Runtime actions backing the shared ExtensionRuntime (all pi.* action methods). */
export function createBridgeActions({ output, sendRequest, session, state }) {
	const extractText = (content) =>
		typeof content === "string" ? content : (content ?? []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
	const extractImages = (content) => (typeof content === "string" ? [] : (content ?? []).filter((c) => c.type === "image"));

	return {
		// Custom messages surface as notify (the F&F UI channel for extension user-facing messages).
		sendMessage: (message) =>
			output({ type: "extension_ui_request", id: crypto.randomUUID(), method: "notify", message: extractText(message.content), notifyType: "info" }),
		sendUserMessage: (content) => {
			const images = extractImages(content);
			output({ type: "user_message", text: extractText(content), ...(images.length ? { images } : {}) });
		},
		appendEntry: (customType, data) => {
			session.sessionManager.appendCustomEntry(customType, data);
			// The SessionEntry keeps its own `type` (e.g. "custom"); it is nested so the
			// line's event discriminator stays `session_append_entry` on the wire.
			output({ type: "session_append_entry", entry: session.view.entries.at(-1) });
		},
		setSessionName: (name) => {
			session.sessionManager.appendSessionInfo(name);
			output({ type: "session_info_changed", name: session.view.name });
		},
		getSessionName: () => session.view.name,
		setLabel: (entryId, label) => {
			if (label === undefined) session.view.labels.delete(entryId);
			else session.view.labels.set(entryId, label);
		},
		getActiveTools: () => [...state.activeTools],
		getAllTools: () => state.allToolInfo,
		getSettings: () => ({}),
		setActiveTools: (names) => {
			state.activeTools = new Set(names);
		},
		// v0: the tool set is frozen after ready; dynamic registration refresh is a no-op.
		refreshTools: () => {},
		getCommands: () => state.commands.map((c) => ({ name: c.name, description: c.description, source: "extension", sourceInfo: c.sourceInfo ?? {} })),
		setModel: async (model) => {
			const res = await sendRequest({ type: "set_model_request", modelId: model?.id, provider: model?.provider });
			if (res.success && res.data?.ok) {
				session.view.model = model;
				session.sessionManager.appendModelChange(model?.provider, model?.id);
				return true;
			}
			log(`set_model_request for ${model?.provider}/${model?.id} not applied by core`);
			return false;
		},
		getThinkingLevel: () => session.view.thinkingLevel,
		setThinkingLevel: (level) => {
			session.view.thinkingLevel = level;
			session.sessionManager.appendThinkingLevelChange(level);
			void sendRequest({ type: "set_thinking_level_request", level }).then((res) => {
				if (!res.success || !res.data?.ok) log(`set_thinking_level_request for "${level}" not applied by core`);
			});
		},
	};
}
