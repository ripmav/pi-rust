/**
 * WP-701 e2e fixture variant: provider "faketest-error" whose streamSimple
 * terminates directly with a terminal `error` event (upstream contract:
 * stopReason "error" + errorMessage). The core must surface it as a model
 * error (run fails, error message on stderr).
 */
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai/compat";

export default function (pi: any) {
	pi.registerProvider("faketest-error", {
		api: "faketest-error-api",
		models: [
			{
				id: "faketest-error/boom-1",
				name: "Fake Boom",
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 8192,
				maxTokens: 1024,
			},
		],
		streamSimple(model: any) {
			const stream = createAssistantMessageEventStream();
			(async () => {
				stream.push({
					type: "error",
					reason: "error",
					error: {
						role: "assistant",
						content: [],
						api: model?.api,
						provider: model?.provider,
						model: model?.id,
						usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
						stopReason: "error",
						errorMessage: "boom from fake provider",
						timestamp: Date.now(),
					},
				});
				stream.end();
			})();
			return stream;
		},
	});
}
