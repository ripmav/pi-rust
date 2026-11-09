/**
 * test-only fixture, not an unmodified upstream extension
 *
 * Registers a `crash` tool whose execute() kills the sidecar process,
 * exercising the core's degraded state machine: sidecar crash mid
 * extension_tool_call, built-ins must keep working, exit code 3.
 */
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const crashTool = defineTool({
	name: "crash",
	label: "Crash",
	description: "Test fixture: kill the sidecar process",
	parameters: Type.Object({}),

	async execute() {
		process.exit(1);
	},
});

export default function (pi: ExtensionAPI) {
	pi.registerTool(crashTool);
}
