/**
 * test-only fixture, not an unmodified upstream extension
 *
 * WP-1004 (codemode hosting): `echo_param` (direct, active) is callable from
 * inside a codemode script via `tools.echo_param(...)` — the nested call
 * round-trips through the bridge (tool_call_request → core → normal
 * tool_call decision → back into the sandbox).
 */
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	pi.registerTool(
		defineTool({
			name: "echo_param",
			label: "Echo Param",
			description: "Test fixture: echoes the text parameter",
			parameters: Type.Object({
				text: Type.String({ description: "Text to echo back" }),
			}),
			async execute(_toolCallId, params) {
				return { content: [{ type: "text", text: `echo-param:${params.text}` }] };
			},
		}),
	);
}
