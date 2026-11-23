/**
 * Regression e2e fixture: a STRICT extension provider — like the real
 * pi-ai streamers it wraps, it reads the shape of every conversation
 * message. The old core bug (before_agent_start custom messages ingested
 * raw: no role, string content) crashed pi-ai with "Cannot read properties
 * of undefined (reading 'length')"; this fixture encodes the inspection
 * result in the reply text instead of throwing, so the test asserts the
 * core sent an LLM-safe conversation and the injected custom message
 * arrived as a user message.
 *
 * Registered in the v0-bridge legacy config form (two-arg
 * `registerProvider(name, config)`), the same shape a user writes for a
 * real OpenAI-compatible endpoint.
 */
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai/compat";

const U0 = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

function assistant(content: any[], stopReason: string) {
	return { role: "assistant", content, usage: U0, stopReason, timestamp: Date.now() };
}

export default function (pi: any) {
	// v0 bridge: two-arg legacy config form (name + config object).
	pi.registerProvider("llmsanity", {
		baseUrl: "https://llmsanity.invalid/v1",
		apiKey: "sanity-key",
		api: "llmsanity-api",
		models: [
			{
				id: "check-1",
				name: "LLM Sanity Check",
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 8192,
				maxTokens: 1024,
			},
		],
		streamSimple(_model: any, context: any) {
			const stream = createAssistantMessageEventStream();
			const messages: any[] = context?.messages ?? [];
			// Strict, like pi-ai: touch the shape of every message.
			const badRoles = messages
				.filter((m) => !["system", "user", "assistant", "toolResult"].includes(m?.role))
				.map((m) => String(m?.role));
			const markerUsers = messages.filter(
				(m) => m?.role === "user" && JSON.stringify(m?.content ?? "").includes("CUSTOM-NOTE-42"),
			);
			const text = `LLMSANITY badRoles=${JSON.stringify(badRoles)} markerUserMessages=${markerUsers.length}`;
			const partial = assistant([{ type: "text", text }], "stop");
			(async () => {
				stream.push({ type: "start", partial: assistant([], "stop") });
				stream.push({ type: "text_delta", contentIndex: 0, delta: text, partial });
				stream.push({ type: "done", reason: "stop", message: partial });
				stream.end();
			})();
			return stream;
		},
	});
}
