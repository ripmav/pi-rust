/**
 * WP-1005 e2e fixture: registers the fake provider "fakenative" in the
 * NATIVE Provider object form (what the unmodified llama extension
 * registers). auth.apiKey.resolve returns a key from env-style resolution
 * (no env needed here — the key is baked in to prove the sidecar merged the
 * resolved auth into the stream options). streamSimple echoes the received
 * apiKey in the reply text. Also registers `switch_to_native`, a tool that
 * pi.setModel's to the extension-registered model.
 */
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai/compat";

const U0 = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

function assistant(content: any[], usage: any, stopReason: string) {
	return { role: "assistant", content, usage, stopReason, timestamp: Date.now() };
}

function scripted(apiKey: string | undefined) {
	const stream = createAssistantMessageEventStream();
	const text = `native stream, key=${apiKey ?? "missing"}`;
	const partial = assistant([{ type: "text", text }], U0, "stop");
	(async () => {
		stream.push({ type: "start", partial: assistant([], U0, "stop") });
		stream.push({ type: "text_delta", contentIndex: 0, delta: text, partial });
		stream.push({
			type: "done",
			reason: "stop",
			message: assistant([{ type: "text", text }], { ...U0, input: 5, output: 6, totalTokens: 11 }, "stop"),
		});
		stream.end();
	})();
	return stream;
}

export default function (pi: any) {
	pi.registerProvider({
		id: "fakenative",
		name: "Fake Native",
		auth: {
			apiKey: {
				name: "fake native server",
				resolve: async () => ({
					auth: { apiKey: "native-key-ok" },
					source: "fixture",
				}),
			},
		},
		getModels: () => [
			{
				id: "fakenative/echo-1",
				name: "Fake Native Echo",
				api: "fakenative-api",
				provider: "fakenative",
				baseUrl: "https://fakenative.invalid/v1",
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 8192,
				maxTokens: 1024,
			},
		],
		streamSimple(_model: any, _context: any, options?: any) {
			return scripted(options?.apiKey);
		},
	});

	pi.registerTool({
		name: "switch_to_native",
		label: "Switch to Native",
		description: "WP-1005 fixture: pi.setModel to the native-form extension model",
		parameters: { type: "object", properties: {} },
		async execute() {
			const ok = await pi.setModel({ id: "fakenative/echo-1", provider: "fakenative" });
			return { content: [{ type: "text", text: `switched:${ok}` }], details: { ok } };
		},
	});
}
