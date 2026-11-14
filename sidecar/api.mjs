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

export function createDialogManager({ output, uiMethods, defaultTimeoutMs, sendRequest }) {
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
		setTheme: (name) => {
			// WP-607: the core applies the switch in TUI mode (it validates
			// the theme name and answers); headless runs reject it.
			return sendRequest({ type: "set_theme_request", name }).then(
				(res) => (res?.success ? { success: true } : { success: false, error: res?.error ?? "theme switch failed" }),
				(e) => ({ success: false, error: String(e) }),
			);
		},
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
export function createBridgeActions({ output, sendRequest, session, state, onActiveToolsChanged }) {
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
		// WP-1003 (protocol v5): semantics + wire notification live in the
		// bridge (applyActiveTools) — unknown and hidden tools ignored, then
		// the core receives tools_activated.
		setActiveTools: (names) => {
			onActiveToolsChanged?.(names);
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
		// Gated like setModel: the sidecar state and the session log only move once
		// the core confirmed the change (an unconfirmed optimistic update would
		// claim a change the core never applied).
		setThinkingLevel: async (level) => {
			const res = await sendRequest({ type: "set_thinking_level_request", level });
			if (res.success && res.data?.ok) {
				session.view.thinkingLevel = level;
				session.sessionManager.appendThinkingLevelChange(level);
				return true;
			}
			log(`set_thinking_level_request for "${level}" not applied by core`);
			return false;
		},
	};
}

/**
 * WP-1006: the ctx.ui.custom pane host. The factory's real pi-tui component
 * runs sidecar-side; the pane host translates the component tree into the
 * structured descriptor (WP-707 subset: title, lines, list, input, blocked),
 * relays keys into component.handleInput, and pushes pane_update on
 * requestRender / after keys (change-only, signature-deduped). done(result)
 * resolves the factory's promise and emits pane_close.
 *
 * Out-of-subset components degrade: no recognizable SelectList/Input/Text
 * tree → the raw render(width) lines (ANSI-stripped) are relayed as `lines`;
 * a component that throws at construction → typed error, no crash.
 */
const PANE_KEY_ENCODINGS = {
	up: "\u001b[A",
	down: "\u001b[B",
	right: "\u001b[C",
	left: "\u001b[D",
	enter: "\r",
	escape: "\u001b",
	backspace: "\u007f",
	tab: "\t",
};

export function createPaneHost({ output, tui, keybindings, theme }) {
	const panes = new Map(); // id -> { component, signature, resolve }
	let seq = 0;

	// Walk the pi-tui component tree into the descriptor subset.
	function descriptorOf(component, width) {
		const out = { title: undefined, lines: [] };
		let foundSelectable = false;
		const walk = (comp) => {
			if (!comp || typeof comp !== "object") return;
			if (comp instanceof tui.SelectList) {
				out.list = {
					items: (comp.filteredItems ?? comp.items ?? []).map((it) =>
						typeof it === "string" ? it : (it?.value ?? String(it)),
					),
					cursor: comp.selectedIndex ?? 0,
				};
				foundSelectable = true;
				return;
			}
			if (comp instanceof tui.Input) {
				out.input = { label: comp.prompt ?? "", value: comp.getValue?.() ?? comp.value ?? "" };
				if (comp.placeholder) out.input.placeholder = comp.placeholder;
				foundSelectable = true;
				return;
			}
			if (comp instanceof tui.Text) {
				const text = typeof comp.text === "string" ? comp.text : "";
				if (text) {
					if (out.lines.length === 0 && !out.title) out.title = text;
					out.lines.push({ text });
				}
				return;
			}
			if (Array.isArray(comp.children)) for (const child of comp.children) walk(child);
		};
		walk(component);
		if (!foundSelectable && out.lines.length === 0 && typeof component.render === "function") {
			// Out-of-subset: relay the rendered lines (ANSI stripped).
			for (const line of component.render(width) ?? []) {
				const text = tui.stripTerminalSequences?.(line) ?? line;
				if (text) out.lines.push({ text });
			}
		}
		if (out.lines.length === 0 && !out.list) out.lines = [];
		return out;
	}

	function pushUpdate(id, width) {
		const pane = panes.get(id);
		if (!pane) return;
		let desc;
		try {
			desc = descriptorOf(pane.component, width);
		} catch (err) {
			log(`pane ${id} render failed: ${err instanceof Error ? err.message : String(err)}`);
			return;
		}
		const signature = JSON.stringify({ ...desc, blocked: undefined });
		if (signature === pane.signature) return;
		pane.signature = signature;
		output({ type: "pane_update", id, ...desc });
	}

	function close(id) {
		const pane = panes.get(id);
		if (!pane) return;
		pane.component.dispose?.();
		panes.delete(id);
		output({ type: "pane_close", id });
		pane.resolve(undefined);
	}

	// The real TUI passes keys to the *focused* component (usually the
	// SelectList inside a framed container, or the custom component itself).
	// Mirror that: root handleInput first, then the first capturesViewportInput
	// descendant, then any handleInput descendant (v0: single input target —
	// the production shapes focus one control at a time).
	function inputTarget(component) {
		if (typeof component?.handleInput === "function") return component;
		const queue = [component];
		while (queue.length > 0) {
			const comp = queue.shift();
			if (!comp || typeof comp !== "object") continue;
			if (comp.capturesViewportInput && typeof comp.handleInput === "function") return comp;
			if (Array.isArray(comp.children)) queue.push(...comp.children);
		}
		queue.length = 0;
		queue.push(component);
		while (queue.length > 0) {
			const comp = queue.shift();
			if (!comp || typeof comp !== "object" || comp === component) continue;
			if (typeof comp.handleInput === "function") return comp;
			if (Array.isArray(comp.children)) queue.push(...comp.children);
		}
		return null;
	}

	const host = {
		hasPane: (id) => panes.has(id),
		key(id, key) {
			const pane = panes.get(id);
			if (!pane) return;
			const data = PANE_KEY_ENCODINGS[key?.code] ?? (key?.text ?? "");
			if (!data) return;
			const target = inputTarget(pane.component);
			try {
				target?.handleInput(data);
			} catch (err) {
				log(`pane ${id} handleInput failed: ${err instanceof Error ? err.message : String(err)}`);
			}
			pushUpdate(id, host.width);
		},
		requestRender: () => {
			for (const id of panes.keys()) pushUpdate(id, host.width);
		},
		// The factory-facing API. Resolves when done(result) fires (or the
		// pane is closed without a result).
		custom(factory, options) {
			const id = `pane_${++seq}`;
			return new Promise(async (resolve) => {
				const fakeTui = { requestRender: () => host.requestRender() };
				let component;
				try {
					component = await factory(fakeTui, theme, keybindings, (result) => {
						const pane = panes.get(id);
						if (pane) pane.signature = ""; // allow a final update
						resolve(result);
						close(id);
					});
				} catch (err) {
					// Typed graceful degradation (WP-1006 acceptance).
					log(`ui.custom factory failed: ${err instanceof Error ? err.message : String(err)}`);
					resolve(undefined);
					return;
				}
				panes.set(id, { component, signature: "" });
				output({
					type: "pane_open",
					id,
					...(options?.blocked ? { blocked: options.blocked } : {}),
				});
				pushUpdate(id, host.width);
			});
		},
		width: 100,
	};
	return host;
}
