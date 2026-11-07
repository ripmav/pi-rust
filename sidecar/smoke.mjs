#!/usr/bin/env node
/**
 * Smoke test: a fake core over pipes that drives the sidecar end-to-end.
 *
 * Loads five UNMODIFIED upstream example extensions (hello.ts, with-deps/,
 * permission-gate.ts, event-bus.ts, claude-rules.ts) plus one scratch test
 * fixture (created in a temp dir) that exercises sendUserMessage /
 * appendEntry / setSessionName / registerFlag.
 */
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const EXAMPLES = path.join(ROOT, "upstream/packages/coding-agent/examples/extensions");
const ENTRY = path.join(path.dirname(fileURLToPath(import.meta.url)), "index.mjs");

let passed = 0;
let failed = 0;
function check(name, cond, detail = "") {
	if (cond) {
		passed++;
		console.error(`ok   - ${name}`);
	} else {
		failed++;
		console.error(`FAIL - ${name}${detail ? ` :: ${JSON.stringify(detail).slice(0, 400)}` : ""}`);
	}
}

/** Fake core: NDJSON client over the sidecar's stdin/stdout. */
class FakeCore {
	constructor(child) {
		this.child = child;
		this.pending = new Map(); // id -> resolve(response)
		this.events = [];
		this.waiters = [];
		this.buf = "";
		this.exited = new Promise((resolve) => child.on("exit", (code) => resolve(code)));
		child.stdout.on("data", (chunk) => this.onData(chunk.toString()));
	}
	send(obj) {
		this.child.stdin.write(JSON.stringify(obj) + "\n");
	}
	sendRaw(line) {
		this.child.stdin.write(line + "\n");
	}
	request(payload, timeoutMs = 20000) {
		const id = crypto.randomUUID();
		this.send({ id, ...payload });
		return this.responseFor(id, timeoutMs);
	}
	responseFor(id, timeoutMs = 20000) {
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.pending.delete(id);
				reject(new Error(`timeout waiting for response ${id}`));
			}, timeoutMs);
			this.pending.set(id, (response) => {
				clearTimeout(timer);
				resolve(response);
			});
		});
	}
	/** Wait for the next unconsumed event matching pred. */
	nextEvent(pred, what, timeoutMs = 20000) {
		const find = () => {
			const i = this.events.findIndex(pred);
			return i === -1 ? undefined : this.events.splice(i, 1)[0];
		};
		const immediate = find();
		if (immediate) return Promise.resolve(immediate);
		return new Promise((resolve, reject) => {
			const timer = setTimeout(() => {
				this.waiters.splice(this.waiters.indexOf(wake), 1);
				reject(new Error(`timeout waiting for event: ${what}`));
			}, timeoutMs);
			const wake = () => {
				const ev = find();
				if (ev) {
					clearTimeout(timer);
					resolve(ev);
				}
			};
			this.waiters.push(wake);
		});
	}
	onData(chunk) {
		this.buf += chunk;
		let i;
		while ((i = this.buf.indexOf("\n")) !== -1) {
			const line = this.buf.slice(0, i).replace(/\r$/, "");
			this.buf = this.buf.slice(i + 1);
			if (!line.trim()) continue;
			const obj = JSON.parse(line);
			if (obj.type === "response" && obj.id != null && this.pending.has(obj.id)) {
				const waiter = this.pending.get(obj.id);
				this.pending.delete(obj.id);
				waiter(obj);
			} else {
				// response lines for unknown/null ids (e.g. the parse error) are observable events here
				this.events.push(obj);
				for (const wake of [...this.waiters]) wake();
			}
		}
	}
}

const SCRATCH_EXTENSION = `
export default function (pi) {
	pi.registerFlag("probe", { type: "boolean", default: true });
	pi.registerTool({
		name: "probe_send",
		label: "Probe Send",
		description: "Smoke fixture: emits bridge state mutations",
		parameters: { type: "object", properties: {} },
		async execute() {
			pi.sendUserMessage("hello from extension");
			pi.appendEntry("probe", { n: 1 });
			pi.setSessionName("smoke-session");
			return { content: [{ type: "text", text: "probed" }], details: { ok: true } };
		},
	});
}
`;

async function main() {
	const tmp = mkdtempSync(path.join(tmpdir(), "pi-bridge-smoke-"));
	const extDir = path.join(tmp, "probe-ext");
	mkdirSync(extDir, { recursive: true });
	writeFileSync(path.join(extDir, "index.ts"), SCRATCH_EXTENSION);
	mkdirSync(path.join(tmp, ".claude/rules"), { recursive: true });
	writeFileSync(path.join(tmp, ".claude/rules/testing.md"), "# testing rules\n");
	const agentDir = path.join(tmp, "agent");
	mkdirSync(agentDir, { recursive: true }); // isolate discovery from the user's global extensions

	const child = spawn(
		process.execPath,
		[
			ENTRY,
			"--extension-dir", path.join(EXAMPLES, "hello.ts"),
			"--extension-dir", path.join(EXAMPLES, "with-deps"),
			"--extension-dir", path.join(EXAMPLES, "permission-gate.ts"),
			"--extension-dir", path.join(EXAMPLES, "event-bus.ts"),
			"--extension-dir", path.join(EXAMPLES, "claude-rules.ts"),
			"--extension-dir", extDir,
			"--agent-dir", agentDir,
		],
		{ cwd: tmp, stdio: ["pipe", "pipe", "pipe"] },
	);
	child.stderr.on("data", (d) => process.stderr.write(d));
	const core = new FakeCore(child);

	try {
		// (a) handshake
		const init = await core.request({
			type: "init",
			protocolVersion: 0,
			capabilities: { ui: ["select", "confirm", "input", "editor", "notify"], timeoutMs: 5000 },
		});
		check("init succeeds, apiVersion 0", init.success === true && init.data?.apiVersion === 0, init);

		// (b) ready registry contents
		const ready = await core.nextEvent((e) => e.type === "ready", "ready");
		const toolNames = ready.tools.map((t) => t.name);
		check("ready tools incl. with-deps npm-dep proof", toolNames.includes("hello") && toolNames.includes("parse_duration"), toolNames);
		const hello = ready.tools.find((t) => t.name === "hello");
		check("hello keeps TypeBox schema", hello?.parameters?.type === "object" && hello?.parameters?.properties?.name?.type === "string", hello?.parameters);
		check("ready commands incl. emit", ready.commands.some((c) => c.name === "emit"), ready.commands);
		check("ready flags incl. probe", ready.flags.some((f) => f.name === "probe" && f.default === true), ready.flags);
		check(
			"ready events incl. tool_call/session_start/before_agent_start",
			["tool_call", "session_start", "before_agent_start"].every((ev) => ready.events.includes(ev)),
			ready.events,
		);
		check("ready lists 6 extensions, no errors", ready.extensions.length === 6 && ready.errors.length === 0, ready.errors);

		// (c) session_start: pi.events round-trip + claude-rules scan
		core.send({ type: "session_start", event: { type: "session_start", reason: "startup" } });
		const busNotify = await core.nextEvent(
			(e) => e.type === "extension_ui_request" && e.method === "notify" && /Session started/.test(e.message),
			"event-bus notify",
		);
		check("pi.events round-trip emits notify", /Event from event-bus-example: Session started/.test(busNotify.message), busNotify.message);
		const rulesNotify = await core.nextEvent(
			(e) => e.type === "extension_ui_request" && e.method === "notify" && /rule/.test(e.message),
			"claude-rules notify",
		);
		check("claude-rules found rule files", /Found 1 rule/.test(rulesNotify.message), rulesNotify.message);

		// (d) tool_call decision: permission-gate must block (user declines via select)
		core.send({
			id: "tc-decision",
			type: "tool_call",
			event: { type: "tool_call", toolName: "bash", toolCallId: "tc_1", input: { command: "rm -rf /" } },
		});
		const select = await core.nextEvent(
			(e) => e.type === "extension_ui_request" && e.method === "select",
			"permission-gate select",
		);
		check("gate asked for confirmation", /rm -rf/.test(select.title), select.title);
		core.send({ type: "extension_ui_response", id: select.id, cancelled: true });
		const blocked = await core.responseFor("tc-decision");
		check("tool_call blocked with reason", blocked.data?.result?.block === true && /Blocked by user/.test(blocked.data.result.reason), blocked);
		check("tool_call input unchanged", blocked.data?.input?.command === "rm -rf /", blocked.data?.input);

		// non-dangerous bash passes through
		const allowed = await core.request({
			type: "tool_call",
			event: { type: "tool_call", toolName: "bash", toolCallId: "tc_1b", input: { command: "echo ok" } },
		});
		check("non-dangerous tool_call not blocked", allowed.success && allowed.data?.result === null && allowed.data?.input?.command === "echo ok", allowed);

		// (e) extension_tool_call round-trips with AgentToolResult shape
		const helloCall = await core.request({ type: "extension_tool_call", toolCallId: "tc_2", toolName: "hello", input: { name: "Bridge" } });
		check(
			"hello tool AgentToolResult",
			helloCall.success && helloCall.data?.content?.[0]?.text === "Hello, Bridge!" && helloCall.data?.details?.greeted === "Bridge",
			helloCall,
		);		const durationCall = await core.request({ type: "extension_tool_call", toolCallId: "tc_3", toolName: "parse_duration", input: { duration: "1h" } });
		check("parse_duration executes (ms resolved)", durationCall.data?.content?.[0]?.text === "1h = 3600000 milliseconds", durationCall);
		const unknownTool = await core.request({ type: "extension_tool_call", toolCallId: "tc_9", toolName: "nope", input: {} });
		check("unknown extension tool errors", unknownTool.success === false && /Unknown extension tool/.test(unknownTool.error), unknownTool);

		// (f) sidecar->core mutations triggered via a registered tool
		const probe = await core.request({ type: "extension_tool_call", toolCallId: "tc_4", toolName: "probe_send", input: {} });
		check("probe tool executed", probe.data?.content?.[0]?.text === "probed" && probe.data?.details?.ok === true, probe);
		const userMessage = await core.nextEvent((e) => e.type === "user_message", "user_message");
		check("user_message emitted", userMessage.text === "hello from extension", userMessage);
		const appended = await core.nextEvent((e) => e.type === "session_append_entry", "session_append_entry");
		check("session_append_entry emitted", appended.entry?.customType === "probe" && appended.entry?.data?.n === 1 && appended.entry?.type === "custom", appended);
		const renamed = await core.nextEvent((e) => e.type === "session_info_changed", "session_info_changed");
		check("session_info_changed emitted", renamed.name === "smoke-session", renamed);

		// registered command invocation (bridge v0.1 addition, see README)
		const cmd = await core.request({ type: "command", name: "emit", args: "smoke" });
		check("command invoke succeeds", cmd.success === true, cmd);
		const cmdNotify = await core.nextEvent(
			(e) => e.type === "extension_ui_request" && e.method === "notify" && /smoke/.test(e.message),
			"command-triggered notify",
		);
		check("command -> pi.events -> notify", /Event from \/emit command: smoke/.test(cmdNotify.message), cmdNotify.message);

		// (g) before_agent_start: claude-rules rewrites the system prompt
		const bas = await core.request({
			type: "before_agent_start",
			event: { type: "user", messageType: "user", prompt: "hi" },
			systemPrompt: "BASE PROMPT",
		});
		check(
			"before_agent_start returns rendered systemPrompt",
			typeof bas.data?.systemPrompt === "string" &&
				bas.data.systemPrompt.startsWith("BASE PROMPT") &&
				/Project Rules/.test(bas.data.systemPrompt) &&
				/\.claude\/rules\/testing\.md/.test(bas.data.systemPrompt),
			bas.data,
		);

		// (h) fast paths + error model. Decision events with no handler answer success with data: null.
		const fastPaths = [
			["user_bash", { type: "user_bash", command: "echo x", excludeFromContext: false, cwd: tmp }],
			["input", { type: "input", text: "hello", source: "rpc" }],
			["tool_result", { type: "tool_result", toolName: "bash", toolCallId: "tc_1b", input: { command: "echo ok" }, content: [{ type: "text", text: "ok" }], isError: false }],
			// Two-phase emitContext contract: the sidecar transforms nothing on `context` (always null);
			// handler phases run once on `context_with_system`.
			["context", { type: "context", messages: [] }],
			["context_with_system", { type: "context_with_system", messages: [{ role: "user", content: [{ type: "text", text: "hi" }] }] }],
		];
		for (const [type, event] of fastPaths) {
			const r = await core.request({ type, event });
			check(`${type} fast path null`, r.success && r.data === null, r);
		}
		core.sendRaw("this is not json {");
		const parseErr = await core.nextEvent((e) => e.type === "response" && e.command === "parse", "parse error");
		check("malformed line -> parse error", parseErr.success === false && /Failed to parse command/.test(parseErr.error), parseErr);
		const unknown = await core.request({ type: "bogus_command" });
		check("unknown command error", unknown.success === false && /Unknown command: bogus_command/.test(unknown.error), unknown);

		// (i) clean shutdown
		core.send({ type: "session_shutdown", event: { type: "session_shutdown", reason: "quit" } });
		const code = await Promise.race([core.exited, new Promise((r) => setTimeout(() => r("timeout"), 10000))]);
		check("clean exit 0 after session_shutdown", code === 0, code);
	} catch (err) {
		failed++;
		console.error(`FAIL - smoke aborted: ${err.message}`);
		try {
			child.kill("SIGKILL");
		} catch {}
	}

	rmSync(tmp, { recursive: true, force: true }); // teardown: remove our temp fixtures

	console.error(`\nsmoke: ${passed} passed, ${failed} failed`);
	process.exit(failed === 0 ? 0 : 1);
}

main();
