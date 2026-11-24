/**
 * E3S3 e2e fixture: records what the model is declared and what the
 * extension side observes, via pi.appendEntry (the core persists them as
 * `custom` session entries the e2e asserts on):
 * - before_agent_start -> customType "wp1403-selected-tools"
 *   data { selectedTools } (the tool set declared to the model — the
 *   exposure rule: `direct` in, `codemode`/`deferred` out)
 * - mcp_servers_change -> customType "wp1403-mcp-change"
 *   data { servers } (the core forwards its configured servers to the
 *   sidecar after session_start, so subscribing extensions still see the
 *   change — D13 coexistence)
 *
 * Test-only fixture, not an unmodified upstream extension.
 */
export default function (pi) {
	pi.on("before_agent_start", (event) => {
		const tools = (event.systemPromptOptions?.selectedTools ?? []) as string[];
		pi.appendEntry("wp1403-selected-tools", { selectedTools: tools });
	});
	pi.on("mcp_servers_change", (event) => {
		pi.appendEntry("wp1403-mcp-change", {
			servers: ((event.servers ?? []) as Array<{ name: string }>).map((s) => s.name),
		});
	});
}
