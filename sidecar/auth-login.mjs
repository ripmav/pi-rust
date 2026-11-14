#!/usr/bin/env node
/**
 * pi-auth-login (WP-1007) — runs the pinned upstream's `auth login` flow
 * UNMODIFIED (`ModelRuntime.create` + `runAuthLogin`, the same entry the
 * upstream CLI uses). The terminal is relayed to the core over stdio as
 * NDJSON, because the core owns the user's TTY:
 *
 *   sidecar -> core:
 *     {"type":"write","line":string}        plain line to stdout
 *     {"type":"error","line":string}        plain line to stderr
 *     {"type":"prompt","id":string,"kind":"question"|"secret","message":string}
 *     {"type":"open_url","url":string}      the core opens/prints the URL
 *     {"type":"exit","code":number}
 *   core -> sidecar:
 *     {"id":string,"answer":string}         answer to a prompt
 *
 * Usage: node sidecar/auth-login.mjs --agent-dir <path> <provider> [--api-key] [--no-browser]
 *
 * PI_CODING_AGENT_DIR is set to the passed agent dir before any upstream
 * import resolves it, so credentials land in the exact auth.json the Rust
 * core reads. Key material only ever travels inside prompt answers
 * (core -> sidecar) and the auth.json write; it is never logged here.
 */
import path from "node:path";
import { fileURLToPath } from "node:url";

const DIST_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "upstream", "packages", "coding-agent", "dist");

const args = process.argv.slice(2);
let agentDir;
const loginArgs = [];
for (let i = 0; i < args.length; i++) {
	const arg = args[i];
	if (arg === "--agent-dir") {
		agentDir = args[++i];
	} else {
		loginArgs.push(arg);
	}
}
if (!agentDir) {
	console.error("error: --agent-dir <path> is required");
	process.exit(2);
}
process.env.PI_CODING_AGENT_DIR = agentDir;

const { getAgentDir, SettingsManager } = await import(path.join(DIST_DIR, "index.js"));
const { ModelRuntime } = await import(path.join(DIST_DIR, "core", "model-runtime.js"));
const { runAuthLogin } = await import(path.join(DIST_DIR, "cli", "auth-login.js"));

const log = (line) => console.error(`[auth-login] ${line}`);
const out = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");

// ---- prompt relay: one pending prompt at a time (upstream's flow is
// sequential); the core answers by id.
const pending = new Map(); // id -> { resolve, reject, onAbort }
let promptSeq = 0;

const failPending = () => {
	for (const [id, p] of [...pending]) {
		pending.delete(id);
		p.reject(new Error("Login cancelled"));
	}
};

const prompt = (kind, message, signal) =>
	new Promise((resolve, reject) => {
		if (signal.aborted) {
			reject(new Error("Login cancelled"));
			return;
		}
		const id = `auth_prompt_${++promptSeq}`;
		const onAbort = () => {
			pending.delete(id);
			reject(new Error("Login cancelled"));
		};
		signal.addEventListener("abort", onAbort, { once: true });
		pending.set(id, { resolve, reject, onAbort });
		out({ type: "prompt", id, kind, message });
	});

// stdin answers: {"id","answer"}
process.stdin.on("data", (chunk) => {
	for (const line of chunk.toString("utf8").split("\n")) {
		if (!line.trim()) continue;
		let parsed;
		try {
			parsed = JSON.parse(line);
		} catch {
			log(`ignoring malformed line: ${line.slice(0, 80)}`);
			continue;
		}
		const p = pending.get(parsed.id);
		if (p) {
			pending.delete(parsed.id);
			p.resolve(typeof parsed.answer === "string" ? parsed.answer : "");
		}
	}
});
process.stdin.on("end", failPending); // core went away: fail in-flight prompts

// The terminal the upstream flow drives. `interactive` is the CORE's verdict
// (it checks its own TTY before spawning us); the prompts relay over stdio.
const relayTerminal = {
	interactive: true,
	write: (line) => out({ type: "write", line }),
	error: (line) => out({ type: "error", line }),
	question: (message, signal) => prompt("question", message, signal),
	secret: (message, signal) => prompt("secret", message, signal),
	openUrl: (url) => out({ type: "open_url", url }),
};

const cwd = process.cwd();
const agentDirPath = getAgentDir();
const settingsManager = SettingsManager.create(cwd, agentDirPath, { projectTrusted: false });
const modelRuntime = await ModelRuntime.create({ allowModelNetwork: false, signal: AbortSignal.timeout(15_000) });

let code;
try {
	code = await runAuthLogin(loginArgs, {
		modelRuntime,
		terminal: relayTerminal,
		agentDir: agentDirPath,
		cwd,
		getDeviceId: () => settingsManager.getOrCreateDeviceId(),
		signal: AbortSignal.timeout(30 * 60_000), // the user decides the pace; no self-abort
	});
} catch (err) {
	log(`runAuthLogin threw: ${err instanceof Error ? err.message : String(err)}`);
	code = 1;
}
// A rotated token may still be flushing to auth.json (upstream keeps
// credential writes serialized); wait for it before the process exits.
await modelRuntime.settleCredentialWrites?.();
out({ type: "exit", code });
process.exit(code);
