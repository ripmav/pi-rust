/**
 * WP-1005 e2e fixture: a native-form provider whose env-based auth does NOT
 * resolve (no env set) — the sidecar must answer the core's provider_stream
 * with a clean typed "not configured" error, not a crash. Also registers
 * `switch_to_unconfigured` to route the turn to the provider's (statically
 * known) model.
 */
export default function (pi: any) {
	pi.registerProvider({
		id: "fakeunconfigured",
		name: "Fake Unconfigured",
		auth: {
			apiKey: {
				name: "server that needs an env var",
				// Mirrors the llama pattern: URL from env, else unresolved.
				resolve: async ({ ctx }: any) => {
					const url = await ctx.env("FAKE_UNCONFIGURED_URL");
					if (!url) return undefined;
					return { auth: { apiKey: "k", baseUrl: url }, source: "FAKE_UNCONFIGURED_URL" };
				},
			},
		},
		getModels: () => [
			{
				id: "fakeunconfigured/echo-1",
				name: "Fake Unconfigured Echo",
				api: "fakeunconfigured-api",
				provider: "fakeunconfigured",
				baseUrl: "https://fakeunconfigured.invalid/v1",
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 8192,
				maxTokens: 1024,
			},
		],
		streamSimple() {
			throw new Error("unreachable: auth must fail before the stream");
		},
	});

	pi.registerTool({
		name: "switch_to_unconfigured",
		label: "Switch to Unconfigured",
		description: "WP-1005 fixture: pi.setModel to the unconfigured native model",
		parameters: { type: "object", properties: {} },
		async execute() {
			const ok = await pi.setModel({ id: "fakeunconfigured/echo-1", provider: "fakeunconfigured" });
			return { content: [{ type: "text", text: `switched:${ok}` }], details: { ok } };
		},
	});
}
