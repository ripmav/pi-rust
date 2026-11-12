/**
 * WP-701 e2e fixture: registers the fake provider "faketest" with one model
 * ("faketest/echo-1"). Its streamSimple is deterministic:
 * - no toolResult in the transcript -> text + one `bash` tool call
 * - a toolResult present            -> final text
 * Also registers `switch_to_fake`, a tool that pi.setModel's to the
 * extension-registered model (set_model acceptance).
 * Loaded UNMODIFIED by the real sidecar loader (no bridge special-casing).
 */
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai/compat";

const U0 = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

function assistant(content: any[], usage: any, stopReason: string) {
	return { role: "assistant", content, usage, stopReason, timestamp: Date.now() };
}

export default function (pi: any) {
	pi.registerProvider("faketest", {
		api: "faketest-api",
		baseUrl: "https://faketest.invalid/v1",
		models: [
			{
				id: "faketest/echo-1",
				name: "Fake Echo 1",
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 8192,
				maxTokens: 1024,
			},
		],
		streamSimple(_model: any, context: any, _options?: any) {
			const stream = createAssistantMessageEventStream();
			const sawToolResult = context.messages.some((m: any) => m.role === "toolResult");
			(async () => {
				if (!sawToolResult) {
					// Turn 1: text + one tool call (the round trip executes).
					const textPartial = assistant([{ type: "text", text: "checking with bash" }], U0, "toolUse");
					stream.push({ type: "start", partial: assistant([], U0, "toolUse") });
					stream.push({ type: "text_start", contentIndex: 0, partial: textPartial });
					stream.push({ type: "text_delta", contentIndex: 0, delta: "checking with bash", partial: textPartial });
					stream.push({ type: "text_end", contentIndex: 0, content: "checking with bash", partial: textPartial });
					stream.push({ type: "toolcall_start", contentIndex: 1, partial: textPartial });
					stream.push({
						type: "toolcall_end",
						contentIndex: 1,
						toolCall: { type: "toolCall", id: "fake_tc_1", name: "bash", arguments: { command: "echo fake-provider-ok" } },
						partial: textPartial,
					});
					stream.push({
						type: "done",
						reason: "toolUse",
						message: assistant(
							[
								{ type: "text", text: "checking with bash" },
								{ type: "toolCall", id: "fake_tc_1", name: "bash", arguments: { command: "echo fake-provider-ok" } },
							],
							{ ...U0, input: 7, output: 11, totalTokens: 18 },
							"toolUse",
						),
					});
				} else {
					// Turn 2: final text.
					const textPartial = assistant([{ type: "text", text: "fake provider done" }], U0, "stop");
					stream.push({ type: "start", partial: assistant([], U0, "stop") });
					stream.push({ type: "text_delta", contentIndex: 0, delta: "fake provider done", partial: textPartial });
					stream.push({
						type: "done",
						reason: "stop",
						message: assistant([{ type: "text", text: "fake provider done" }], { ...U0, input: 9, output: 4, totalTokens: 13 }, "stop"),
					});
				}
				stream.end();
			})();
			return stream;
		},
	});

	pi.registerTool({
		name: "switch_to_fake",
		label: "Switch to Fake",
		description: "WP-701 fixture: pi.setModel to the extension-registered model",
		parameters: { type: "object", properties: {} },
		async execute() {
			const ok = await pi.setModel({ id: "faketest/echo-1", provider: "faketest" });
			return { content: [{ type: "text", text: `switched:${ok}` }], details: { ok } };
		},
	});
}
