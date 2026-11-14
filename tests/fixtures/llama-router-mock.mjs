// WP-1005 test fixture: a minimal llama.cpp ROUTER mock (test-only, not an
// unmodified upstream extension). Endpoints:
//   GET  /models                 -> router catalog (one loaded model)
//   GET  /props?model=...&autoload=false -> {} (no thinking template)
//   POST /v1/chat/completions    -> OpenAI-compatible SSE stream
// Usage: node llama-router-mock.mjs <port>
import http from "node:http";

const port = Number(process.argv[2] ?? 0);
if (Number.isNaN(port) || port < 0) {
	console.error("usage: node llama-router-mock.mjs [port]");
	process.exit(2);
}

const server = http.createServer((req, res) => {
	if (req.method === "GET" && req.url.startsWith("/models")) {
		res.writeHead(200, { "content-type": "application/json" });
		res.end(
			JSON.stringify({
				data: [
					{
						id: "gemma-mock",
						status: { value: "loaded" },
						architecture: { input_modalities: ["text"] },
						meta: { n_ctx: 4096 },
					},
				],
			}),
		);
		return;
	}
	if (req.method === "GET" && req.url.startsWith("/props")) {
		res.writeHead(200, { "content-type": "application/json" });
		res.end(JSON.stringify({ models_autoload: false }));
		return;
	}
	if (req.method === "POST" && req.url.startsWith("/v1/chat/completions")) {
		let body = "";
		req.on("data", (c) => (body += c));
		req.on("end", () => {
			// Echo back the last user message so the e2e can assert the
			// transcript actually reached the router.
			let lastUser = "";
			try {
				const parsed = JSON.parse(body);
				for (let i = parsed.messages?.length - 1; i >= 0; i--) {
					const m = parsed.messages[i];
					if (m.role !== "user") continue;
					if (typeof m.content === "string") lastUser = m.content;
					else if (Array.isArray(m.content)) lastUser = m.content.filter((c) => c.type === "text").map((c) => c.text).join(" ");
					if (lastUser) break;
				}
			} catch {
				lastUser = "unparsed";
			}
			const text = `llama-mock: ${lastUser}`;
			res.writeHead(200, { "content-type": "text/event-stream" });
			const chunk = (delta, finish) =>
				`data: ${JSON.stringify({ id: "c1", object: "chat.completion.chunk", choices: [{ index: 0, delta, finish_reason: finish ?? null }] })}\n\n`;
			res.write(chunk({ role: "assistant", content: text }));
			res.write(chunk({}, "stop"));
			res.write("data: [DONE]\n\n");
			res.end();
		});
		return;
	}
	res.writeHead(404, { "content-type": "application/json" });
	res.end(JSON.stringify({ error: "not found" }));
});

server.listen(port, "127.0.0.1", () => console.log(`llama-router-mock listening on ${server.address().port}`));
