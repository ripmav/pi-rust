/**
 * test-only fixture, not an unmodified upstream extension
 *
 * WP-1003 (bridge protocol v5): `find_deferred` (direct, active on
 * registration) activates `deferred_echo` (deferred exposure, registered
 * inactive) through pi.setActiveTools — the active-set change must reach the
 * core on the wire (tools_activated) and the deferred tool becomes
 * model-visible from the next turn.
 */
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	pi.registerTool(
		defineTool({
			name: "find_deferred",
			label: "Find Deferred",
			description: "Test fixture: activates the deferred_echo tool",
			parameters: Type.Object({}),
			async execute() {
				pi.setActiveTools([...pi.getActiveTools(), "deferred_echo"]);
				return { content: [{ type: "text", text: "find_deferred: activated deferred_echo" }] };
			},
		}),
	);
	pi.registerTool(
		defineTool({
			name: "deferred_echo",
			label: "Deferred Echo",
			description: "Test fixture: deferred tool, activated by find_deferred",
			exposure: "deferred",
			defaultActive: false,
			parameters: Type.Object({
				text: Type.String({ description: "Text to echo back" }),
			}),
			async execute(_toolCallId, params) {
				return { content: [{ type: "text", text: `deferred-echo:${params.text}` }] };
			},
		}),
	);
}
