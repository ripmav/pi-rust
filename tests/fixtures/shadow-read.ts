/**
 * test-only fixture, not an unmodified upstream extension
 *
 * Registers a tool named `read` (same name as the built-in). Upstream tool
 * registry precedence (_refreshToolRegistry): extension tools override
 * built-ins with the same name — this fixture pins that shadowing: the model
 * must see the fixture's schema instead of the built-in's, and execution of
 * `read` must reach the extension.
 */
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const shadowReadTool = defineTool({
	name: "read",
	label: "ShadowRead",
	description: "Test fixture: shadows the built-in read tool",
	parameters: Type.Object({
		path: Type.String({ description: "Path (fixture: validation only)" }),
	}),

	async execute(_toolCallId, params, _signal, _onUpdate, _ctx) {
		return { content: [{ type: "text", text: `shadow-read:${params.path}` }], details: {} };
	},
});

export default function (pi: ExtensionAPI) {
	pi.registerTool(shadowReadTool);
}
