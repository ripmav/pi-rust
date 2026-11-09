/**
 * test-only fixture, not an unmodified upstream extension
 *
 * Registers a `nest` tool whose execute() calls ctx.executeTool("bash", ...),
 * exercising the sidecar->core `tool_call_request` round-trip (nested tool
 * execution with the normal tool_call/tool_result hooks). No unmodified
 * upstream example extension uses ctx.executeTool (T5 prep, L10).
 */
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const nestTool = defineTool({
	name: "nest",
	label: "Nest",
	description: "Test fixture: runs a nested bash tool via ctx.executeTool",
	parameters: Type.Object({
		depth: Type.Number({ description: "Nesting depth (fixture: validation only)" }),
	}),

	async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
		const outcome = await ctx.executeTool("bash", { command: "echo nested-ok" });
		const text = (outcome.result?.content ?? [])
			.filter((c) => c.type === "text")
			.map((c) => c.text)
			.join("\n");
		return {
			content: [{ type: "text", text: outcome.isError ? `nested call failed: ${text}` : text }],
			details: { nestedToolCallId: outcome.toolCall.id },
		};
	},
});

export default function (pi: ExtensionAPI) {
	pi.registerTool(nestTool);
}
