#!/usr/bin/env node
/**
 * WP-702 e2e fixture: a minimal MCP server (protocol 2025-03-26) over stdio —
 * NDJSON JSON-RPC 2.0, just enough of the surface for the upstream
 * @earendil-works/pi-mcp client (initialize, tools/list, tools/call, ping).
 *
 * Tools: `echo` (returns its input) and `counter` (process-local counter).
 *
 * `--die-after-first-call`: exit(0) right after the first successful
 * tools/call — pins the upstream crash semantics (no auto-restart; the next
 * call reconnects lazily and gets a fresh process).
 */
import process from "node:process";

const dieAfterFirstCall = process.argv.includes("--die-after-first-call");
let counter = 0;
let calls = 0;

const TOOLS = [
	{
		name: "echo",
		description: "Returns the input text prefixed with echo:",
		inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] },
	},
	{
		name: "counter",
		description: "Increments and returns the process-local counter",
		inputSchema: { type: "object", properties: {} },
	},
];

function send(obj) {
	process.stdout.write(JSON.stringify(obj) + "\n");
}

function handleMessage(msg) {
	if (msg.method === "initialize") {
		send({
			jsonrpc: "2.0",
			id: msg.id,
			result: {
				protocolVersion: "2025-03-26",
				capabilities: { tools: { listChanged: false } },
				serverInfo: { name: "pi-rust-mcp-fixture", version: "1.0.0" },
				instructions: "Fixture MCP server for pi-rust e2e tests",
			},
		});
		return;
	}
	if (msg.method === "notifications/initialized") return; // notification: no response
	if (msg.id === undefined) return; // any other notification
	if (msg.method === "ping") {
		send({ jsonrpc: "2.0", id: msg.id, result: {} });
		return;
	}
	if (msg.method === "tools/list") {
		send({ jsonrpc: "2.0", id: msg.id, result: { tools: TOOLS } });
		return;
	}
	if (msg.method === "tools/call") {
		const { name, arguments: args = {} } = msg.params ?? {};
		if (name === "echo") {
			const text = typeof args.text === "string" ? args.text : "";
			send({ jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: `echo:${text}` }], structuredContent: { echo: text }, isError: false } });
		} else if (name === "counter") {
			counter += 1;
			send({ jsonrpc: "2.0", id: msg.id, result: { content: [{ type: "text", text: `count:${counter}` }], structuredContent: { count: counter }, isError: false } });
		} else {
			send({ jsonrpc: "2.0", id: msg.id, error: { code: -32602, message: `Unknown tool: ${name}` } });
			return;
		}
		calls += 1;
		if (dieAfterFirstCall && calls >= 1) {
			// Flush the answer, then die: the client observes the transport drop.
			process.stdout.write("", () => process.exit(0));
		}
		return;
	}
	send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: `Method not found: ${msg.method}` } });
}

let buf = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
	buf += chunk;
	let i;
	while ((i = buf.indexOf("\n")) !== -1) {
		const line = buf.slice(0, i).replace(/\r$/, "");
		buf = buf.slice(i + 1);
		if (!line.trim()) continue;
		let msg;
		try {
			msg = JSON.parse(line);
		} catch {
			continue;
		}
		handleMessage(msg);
	}
});
process.stdin.on("end", () => process.exit(0));
