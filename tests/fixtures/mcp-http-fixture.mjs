/**
 * WP-703 e2e fixture: a minimal MCP streamable-HTTP mock server.
 *
 * - JSON-RPC 2.0 over POST /mcp (initialize, tools/list, tools/call, ping)
 * - `application/json` responses; when the echo input text is "sse" the
 *   response is a `text/event-stream` stream (single message event) to
 *   exercise the client's stream assembly path
 * - the `echo` result carries the server-side view of the Authorization
 *   header (auth-header pinning without echoing secrets into logs)
 * - `counter` keeps server-side state to prove one live connection
 * - GET is answered with 405 (the client tolerates "no server-to-client
 *   stream"); DELETE is a 204 no-op (session close)
 */
import http from "node:http";

const port = Number(process.argv[2] ?? 0);
let counter = 0;

const server = http.createServer((req, res) => {
	if (req.method === "GET") {
		res.writeHead(405, { "content-type": "application/json" });
		res.end(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32000, message: "no server-to-client stream" } }));
		return;
	}
	if (req.method === "DELETE") {
		res.writeHead(204);
		res.end();
		return;
	}
	if (req.method !== "POST") {
		res.writeHead(405);
		res.end();
		return;
	}
	let body = "";
	req.on("data", (chunk) => (body += chunk));
	req.on("end", () => {
		let msg;
		try {
			msg = JSON.parse(body);
		} catch {
			res.writeHead(400, { "content-type": "application/json" });
			res.end(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } }));
			return;
		}
		const json = (obj, status = 200) => {
			res.writeHead(status, { "content-type": "application/json" });
			res.end(JSON.stringify(obj));
		};
		const error = (code, message) => ({ jsonrpc: "2.0", id: msg.id ?? null, error: { code, message } });

		switch (msg.method) {
			case "initialize":
				return json({
					jsonrpc: "2.0",
					id: msg.id,
					result: {
						protocolVersion: "2024-11-05",
						capabilities: { tools: {} },
						serverInfo: { name: "wp703-mock", version: "0.0.0" },
					},
				});
			case "notifications/initialized":
				return json(undefined, 202);
			case "ping":
				return json({ jsonrpc: "2.0", id: msg.id, result: {} });
			case "tools/list":
				return json({
					jsonrpc: "2.0",
					id: msg.id,
					result: {
						tools: [
							{
								name: "echo",
								description: "Echo text back with the server-side view of the Authorization header",
								inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
							},
							{
								name: "counter",
								description: "Increment and return a server-side counter",
								inputSchema: { type: "object", properties: {} },
							},
						],
					},
				});
			case "tools/call": {
				const { name, arguments: args } = msg.params ?? {};
				const auth = req.headers.authorization ?? "(none)";
				if (name === "counter") {
					counter += 1;
					return json({ jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: `count:${counter}` }] } });
				}
				if (name === "echo") {
					const text = args?.text ?? "";
					const payload = {
						jsonrpc: "2.0",
						id: msg.id,
						result: { content: [{ type: "text", text: `echo:${text} auth:${auth}` }] },
					};
					if (text === "sse") {
						// stream response assembly path
						res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
						res.write(`event: message\ndata: ${JSON.stringify(payload)}\n\n`);
						res.end();
						return;
					}
					return json(payload);
				}
				return json(error(-32601, `unknown tool ${name}`));
			}
			default:
				return json(error(-32601, `method not found: ${msg.method}`));
		}
	});
});

server.listen(port, "127.0.0.1", () => console.log(`listening ${server.address().port}`));
