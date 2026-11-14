/**
 * WP-1005 e2e fixture: routes the turn to the built-in llama provider's
 * mock-router model via pi.setModel. The catalog itself comes from the
 * UNMODIFIED llama extension's refreshModels against the test's mock router
 * (no fixture involvement).
 */
export default function (pi: any) {
	pi.registerTool({
		name: "switch_to_llama",
		label: "Switch to Llama",
		description: "WP-1005 fixture: pi.setModel to the llama.cpp mock model",
		parameters: { type: "object", properties: {} },
		async execute() {
			const ok = await pi.setModel({ id: "gemma-mock", provider: "llama.cpp" });
			return { content: [{ type: "text", text: `switched:${ok}` }], details: { ok } };
		},
	});
}
