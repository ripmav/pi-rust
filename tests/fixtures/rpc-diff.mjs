// WP-802: protocol-diff harness. Spawns upstream `dist/cli.js --mode rpc`
// and pi-rust `--mode rpc` against IDENTICAL mock OpenAI providers, feeds
// both the identical command script, and compares the normalized line
// streams (event type + recursive key structure). Diff must be empty for
// the implemented command (`prompt`).
import http from "node:http";
import { spawn } from "node:child_process";
import os from "node:os";
import path from "node:path";
import fs from "node:fs";

const tmp = process.env.TMPDIR || os.tmpdir();

// ---------------------------------------------------------------- mock
const turn1 = [
	'data: {"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"tc_1","type":"function","function":{"name":"bash","arguments":"{\\"command\\":\\"echo diff-hi\\"}"}}]}}]}',
	'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}',
	"data: [DONE]",
];
const turn2 = [
	'data: {"choices":[{"delta":{"role":"assistant","content":"done"}}]}',
	'data: {"choices":[{"delta":{},"finish_reason":"stop"}]}',
	"data: [DONE]",
];
// Stateless per request: select the turn from the request body (turn 2's
// conversation carries the tool result). Counting requests would desync
// when the two processes' requests interleave or a client retries.
const server = http.createServer((sreq, sres) => {
	// SSE frames are blank-line separated (the pinned upstream parser is
	// spec-strict; a single trailing newline merges the data lines).
	let body = "";
	sreq.on("data", (d) => (body += d));
	sreq.on("end", () => {
		const frames = /"role":\s*"tool"/.test(body) ? turn2 : turn1;
		sres.writeHead(200, { "content-type": "text/event-stream" });
		sres.end(frames.join("\n\n") + "\n\n");
	});
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

// ---------------------------------------------------------------- spawn one side
function side(name, bin, binArgs, extraEnv = {}) {
	const agentDir = path.join(tmp, `agent-${name}`);
	fs.mkdirSync(agentDir, { recursive: true });
	if (name === "up") {
		// The pinned upstream CLI has no OPENAI_BASE_URL hook: route its
		// built-in openai models at the mock through models.json.
		fs.writeFileSync(path.join(agentDir, "models.json"), JSON.stringify({
			providers: {
				openai: {
					baseUrl: base,
					apiKey: "e2e-key",
					// the built-in gpt-4o uses the Responses API; pin the
					// model definition to chat/completions (the mock shape)
					models: [{ id: "gpt-4o", api: "openai-completions" }],
				},
			},
		}));
	}
	const p = spawn(bin, binArgs, {
		cwd: tmp,
		env: { ...process.env, OPENAI_API_KEY: "e2e-key", OPENAI_BASE_URL: base, PI_RUST_BIN: process.env.PI_RUST_BIN, ...extraEnv },
		stdio: ["pipe", "pipe", "pipe"],
	});
	const state = { lines: [], stderr: "", done: false };
	let buf = "";
	p.stdout.on("data", (d) => {
		buf += d.toString();
		let i;
		while ((i = buf.indexOf("\n")) >= 0) {
			const line = buf.slice(0, i).replace(/\r$/, "");
			buf = buf.slice(i + 1);
			if (line.trim()) state.lines.push(line);
		}
	});
	p.stderr.on("data", (d) => (state.stderr += d.toString()));
	p.on("exit", () => (state.done = true));
	return { p, state, agentDir };
}

const common = ["--mode", "rpc", "--provider", "openai", "--model", "gpt-4o"];
const S = {
	// upstream: the agent dir is the PI_CODING_AGENT_DIR env (no --agent-dir flag)
	up: side("up", process.execPath, [process.env.UPSTREAM_CLI, ...common], {
		PI_CODING_AGENT_DIR: path.join(tmp, "agent-up"),
	}),
	// pi-rust: native binary, its own --agent-dir flag
	pi: side("pi", process.env.PI_RUST_BIN, [...common, "--agent-dir", path.join(tmp, "agent-pi")]),
};

for (const k of ["up", "pi"]) {
	S[k].p.stdin.write('{"id":"req_1","type":"prompt","message":"run it"}\n');
}

// ---------------------------------------------------------------- collect until agent_settled (or timeout)
async function collect(s, ms = 45000) {
	const t0 = Date.now();
	while (!s.state.lines.some((l) => { try { return JSON.parse(l).type === "agent_settled"; } catch { return false; } }) && Date.now() - t0 < ms) {
		await new Promise((r) => setTimeout(r, 100));
	}
	await new Promise((r) => setTimeout(r, 500)); // let the tail settle
	return s.state.lines;
}
const [upLines, piLines] = await Promise.all([collect(S.up), collect(S.pi)]);

// ---------------------------------------------------------------- normalize
function keyShape(v) {
	if (Array.isArray(v)) {
		return v.length ? ["[", keyShape(v[0])] : ["[]"];
	}
	if (v && typeof v === "object") {
		return Object.keys(v)
			.sort()
			.map((k) => [k, keyShape(v[k])]);
	}
	return null; // value erased
}
function topKeys(v) {
	if (!v || typeof v !== "object" || Array.isArray(v)) return null;
	return JSON.stringify(Object.keys(v).sort());
}
function normalize(line) {
	const v = JSON.parse(line);
	if (v.type === "message_start" || v.type === "message_end") {
		const m = v.message;
		if (m && m.role === "system") {
			// environment-dependent section keys (skills, docs) are not
			// compared — only the top-level message key structure
			return JSON.stringify({ message: topKeys(m) });
		}
	}
	if (v.type === "agent_end") {
		// The first run message is the environment-dependent system
		// message: compare its top-level keys only.
		const first = v.messages && v.messages[0];
		const firstShape = first && first.role === "system" ? topKeys(first) : keyShape(first);
		return JSON.stringify({
			messages: v.messages && v.messages.length ? ["[", firstShape] : ["[]"],
			willRetry: null,
		});
	}
	return JSON.stringify(keyShape(v));
}

const norm = (lines) => lines.map(normalize);
const a = norm(upLines);
const b = norm(piLines);

// ---------------------------------------------------------------- compare
const max = Math.max(a.length, b.length);
let diff = 0;
for (let i = 0; i < max; i++) {
	if (a[i] !== b[i]) {
		diff++;
		console.log(`DIFF @${i}`);
		console.log(`  up: ${upLines[i]}`);
		console.log(`  pi: ${piLines[i] ?? "<missing>"}`);
	}
}
if (process.env.PRINT_LINES) {
	for (const [name, s] of [["upstream", S.up], ["pi-rust", S.pi]]) {
		console.log(`=== ${name} (${s.state.lines.length} lines) ===`);
		for (const l of s.state.lines) console.log(l.length > 200 ? l.slice(0, 200) + "…" : l);
	}
}
for (const [name, s] of [["upstream", S.up], ["pi-rust", S.pi]]) {
	if (s.state.stderr.trim()) console.log(`--- stderr ${name} ---\n${s.state.stderr}`);
}
S.up.p.kill("SIGTERM");
S.pi.p.kill("SIGTERM");
if (diff > 0) {
	console.log(`\n${diff} line(s) differ`);
	process.exit(1);
}
console.log(`protocol diff empty (${a.length} vs ${b.length} normalized lines)`);
process.exit(0);
