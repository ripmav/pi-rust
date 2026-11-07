/**
 * Minimal in-memory session view for `ctx.sessionManager` reads.
 * The core is the persistence source of truth; the sidecar never writes
 * session files. The buffer only mirrors state the sidecar itself produces
 * (session info, custom entries) plus model/thinking-level echoes from core.
 */
import crypto from "node:crypto";

export function createSessionView(cwd) {
	const view = {
		sessionId: crypto.randomUUID(),
		cwd,
		name: undefined,
		model: undefined,
		thinkingLevel: "off",
		systemPrompt: "",
		entries: [],
		labels: new Map(),
	};
	let counter = 0;
	const nextId = () => `e${Date.now().toString(36)}-${(counter++).toString(36)}`;
	const parent = () => (view.entries.length ? view.entries.at(-1).id : null);
	const push = (entry) => {
		view.entries.push(entry);
		return entry.id;
	};
	const now = () => new Date().toISOString();
	const entry = (fields) => ({ id: nextId(), parentId: parent(), timestamp: now(), ...fields });

	return {
		view,
		sessionManager: {
			getCwd: () => view.cwd,
			getSessionId: () => view.sessionId,
			getSessionFile: () => undefined,
			getSessionDir: () => undefined,
			getSessionName: () => view.name,
			getEntries: () => view.entries.slice(),
			getBranch: () => view.entries.slice(),
			getTree: () => [],
			getLeafId: () => (view.entries.length ? view.entries.at(-1).id : null),
			getLeafEntry: () => view.entries.at(-1),
			getLabel: (id) => view.labels.get(id),
			appendMessage: (message) => push(entry({ type: "message", message })),
			appendCustomEntry: (customType, data) => push(entry({ type: "custom", customType, data })),
			appendSessionInfo: (name) => {
				const sanitized = String(name).replace(/[\r\n]+/g, " ").trim();
				view.name = sanitized || undefined;
				return push(entry({ type: "session_info", name: sanitized }));
			},
			appendModelChange: (provider, modelId) => push(entry({ type: "model_change", provider, modelId })),
			appendThinkingLevelChange: (level) => push(entry({ type: "thinking_level_change", thinkingLevel: level })),
		},
	};
}
