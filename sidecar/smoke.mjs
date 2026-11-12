#!/usr/bin/env node
/**
 * Smoke test: a fake core over pipes that drives the sidecar end-to-end.
 *
 * Loads six UNMODIFIED upstream example extensions (hello.ts, with-deps/,
 * permission-gate.ts, event-bus.ts, claude-rules.ts, prompt-customizer.ts)
 * plus one scratch test fixture (created in a temp dir) that exercises
 * sendUserMessage / appendEntry / setSessionName / registerFlag /
 * setThinkingLevel / registerProvider, and pins the P1 advisory fixes:
 * shutdown dispatch guard, orderly exit on a broken stdout pipe, and the
 * 1 MB stdin line cap. Phase 3 verifies the UNMODIFIED upstream
 * custom-provider-gitlab-duo example loads and registers (701c).
 */
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const ZERO_USAGE = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

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
					// Drop the waiter once satisfied: a lingering resolved waiter would
					// keep stealing later events that match its predicate.
					this.waiters.splice(this.waiters.indexOf(wake), 1);
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
			} else if (obj.type === "set_theme_request") {
				// Test policy: the fake (headless) core rejects theme switches
				// with the upstream-shaped error; the sidecar must surface it.
				this.send({
					id: obj.id,
					type: "response",
					command: obj.type,
					success: false,
					error: `theme ${obj.name} cannot be switched outside interactive (TUI) mode`,
				});
			} else if (obj.type === "set_thinking_level_request") {
				// Test policy: the fake core accepts only "high".
				const ok = obj.level === "high";
				this.send({
					id: obj.id,
					type: "response",
					command: obj.type,
					success: ok,
					data: { ok },
					...(ok ? {} : { error: "test: core declines" }),
				});
			} else {
				// response lines for unknown/null ids (e.g. the parse error) are observable events here
				this.events.push(obj);
				for (const wake of [...this.waiters]) wake();
			}
		}
	}
}

const SCRATCH_EXTENSION = `
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai/compat";

export default function (pi) {
	// WP-701: registered providers (legacy config form). smokeprovider has a
	// deterministic streamSimple ($ENV apiKey resolution is exercised by the
	// provider_stream round trip); smoke-badkey pins the !command rejection.
	const U0 = ${JSON.stringify(ZERO_USAGE)};
	const assistant = (content, usage, stopReason) => ({ role: "assistant", content, usage, stopReason, timestamp: Date.now() });
	pi.registerProvider("smokeprovider", {
		api: "smoke-api",
		apiKey: "$SMOKE_PROVIDER_KEY",
		models: [{ id: "smoke/echo-1", name: "Smoke Echo", input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 512 }],
		streamSimple(model, context, options) {
			const stream = createAssistantMessageEventStream();
			(async () => {
				const text = "key=" + (options?.apiKey ?? "none") + " msgs=" + (context?.messages?.length ?? 0) + " api=" + (model?.api ?? "?");
				const partial = assistant([{ type: "text", text }], U0, "stop");
				stream.push({ type: "start", partial: assistant([], U0, "stop") });
				stream.push({ type: "text_delta", contentIndex: 0, delta: text, partial });
				stream.push({ type: "done", reason: "stop", message: assistant([{ type: "text", text }], { ...U0, input: 2, output: 2, totalTokens: 4 }, "stop") });
				stream.end();
			})();
			return stream;
		},
	});
	pi.registerProvider("smoke-badkey", {
		api: "smoke-bad-api",
		apiKey: "!fake-command",
		models: [{ id: "smoke/bad-1", name: "Smoke Bad", input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 512 }],
		streamSimple() {
			throw new Error("must not run");
		},
	});
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
	pi.registerTool({
		name: "theme_probe",
		label: "Theme Probe",
		description: "Smoke fixture: reports setTheme acceptance",
		parameters: { type: "object", properties: { name: { type: "string" } } },
		async execute(_toolCallId, args, _signal, _onUpdate, ctx) {
			// Upstream contract: ctx.ui.setTheme (synchronous return; the bridge
			// round-trip makes it a Promise — upstream usage is fire-and-forget,
			// this probe awaits it).
			const res = await ctx.ui.setTheme(args.name);
			pi.appendEntry("theme-probe", { success: res.success, error: res.error ?? null });
			return { content: [{ type: "text", text: "probed" }], details: { success: res.success, error: res.error ?? null } };
		},
	});
	pi.registerTool({
		name: "thinking_probe",
		label: "Thinking Probe",
		description: "Smoke fixture: reports setThinkingLevel acceptance",
		parameters: { type: "object", properties: { level: { type: "string" } } },
		async execute(_toolCallId, args) {
			pi.setThinkingLevel(args.level);
			// The upstream loader wrapper drops the return value; wait for the
			// request round-trip, then report the view state (the observable).
			await new Promise((r) => setTimeout(r, 300));
			const after = pi.getThinkingLevel();
			pi.appendEntry("thinking-probe", { after });
			return { content: [{ type: "text", text: "probed" }], details: { after } };
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
			"--extension-dir", path.join(EXAMPLES, "prompt-customizer.ts"),
			"--extension-dir", extDir,
			"--agent-dir", agentDir,
		],
		{ cwd: tmp, stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, SMOKE_PROVIDER_KEY: "smoke-key-123" } },
	);
	child.stderr.on("data", (d) => process.stderr.write(d));
	const core = new FakeCore(child);

	let child2 = null;
	let child3 = null;
	try {
		// (a) handshake
		const init = await core.request({
			type: "init",
			protocolVersion: 1,
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
		check("ready lists 7 extensions, no errors", ready.extensions.length === 7 && ready.errors.length === 0, ready.errors);

		// (b2) WP-701: ready.providers is the object shape [{name, models:[...]}].
		const smokeProv = ready.providers?.find((p) => p?.name === "smokeprovider");
		check(
			"ready providers are objects with model entries",
			Array.isArray(ready.providers) && ready.providers.length === 2 &&
				ready.providers.every((p) => p && typeof p.name === "string" && Array.isArray(p.models)),
			ready.providers,
		);
		check(
			"ready provider model entry carries provider/id/api",
			Boolean(smokeProv?.models?.some((m) => m.provider === "smokeprovider" && m.id === "smoke/echo-1" && m.api === "smoke-api" && m.name === "Smoke Echo")),
			smokeProv,
		);

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

		// (f2) setThinkingLevel: the core's answer gates the sidecar state update
		await core.request({ type: "extension_tool_call", toolCallId: "tc_5", toolName: "thinking_probe", input: { level: "low" } });
		const probeDeclined = await core.nextEvent(
			(e) => e.type === "session_append_entry" && e.entry?.customType === "thinking-probe",
			"thinking probe (declined)",
		);
		check("declined setThinkingLevel leaves view unchanged", probeDeclined.entry?.data?.after === "off", probeDeclined);
		await core.request({ type: "extension_tool_call", toolCallId: "tc_6", toolName: "thinking_probe", input: { level: "high" } });
		const probeAccepted = await core.nextEvent(
			(e) => e.type === "session_append_entry" && e.entry?.customType === "thinking-probe",
			"thinking probe (accepted)",
		);
		check("accepted setThinkingLevel updates view", probeAccepted.entry?.data?.after === "high", probeAccepted);

		// (f3) setTheme: the headless core rejects; the sidecar surfaces the
		// core's error verbatim (upstream {success,error} shape)
		await core.request({ type: "extension_tool_call", toolCallId: "tc_7", toolName: "theme_probe", input: { name: "light" } });
		const probeTheme = await core.nextEvent(
			(e) => e.type === "session_append_entry" && e.entry?.customType === "theme-probe",
			"theme probe",
		);
		check(
			"headless setTheme rejected with core error",
			probeTheme.entry?.data?.success === false &&
				/interactive \(TUI\) mode/.test(probeTheme.entry?.data?.error ?? ""),
			probeTheme,
		);

		// registered command invocation (bridge v0.1 addition, see README)
		const cmd = await core.request({ type: "command", name: "emit", args: "smoke" });
		check("command invoke succeeds", cmd.success === true, cmd);
		check("command data carries handler return value (null when void)", cmd.data === null, cmd);
		const cmdNotify = await core.nextEvent(
			(e) => e.type === "extension_ui_request" && e.method === "notify" && /smoke/.test(e.message),
			"command-triggered notify",
		);
		check("command -> pi.events -> notify", /Event from \/emit command: smoke/.test(cmdNotify.message), cmdNotify.message);

		// (f4) WP-701: provider_stream round trip — the ack, the forwarded
		// events, $ENV apiKey resolution, and the !command typed rejection.
		const streamAck = await core.request({
			type: "provider_stream",
			streamId: "s1",
			model: { provider: "smokeprovider", id: "smoke/echo-1", api: "smoke-api" },
			messages: [{ role: "user", content: [{ type: "text", text: "hi" }] }],
			options: {},
		});
		check("provider_stream acks with the streamId", streamAck.success === true && streamAck.data?.streamId === "s1", streamAck);
		let terminal = null;
		const deltas = [];
		for (let i = 0; i < 20 && !terminal; i++) {
			const ev = await core.nextEvent(
				(e) => e.type === "provider_stream_event" && e.streamId === "s1",
				"provider_stream_event",
				5000,
			);
			if (ev.event?.type === "text_delta") deltas.push(ev.event.delta);
			if (ev.event?.type === "done" || ev.event?.type === "error") terminal = ev.event;
		}
		check(
			"provider stream forwards events with $ENV apiKey resolved",
			deltas.length === 1 && /^key=smoke-key-123 msgs=1 api=smoke-api$/.test(deltas[0]),
			deltas,
		);
		check(
			"provider stream terminates with done + message",
			terminal?.type === "done" && terminal?.message?.content?.[0]?.text === "key=smoke-key-123 msgs=1 api=smoke-api",
			terminal,
		);
		const badKey = await core.request({
			type: "provider_stream",
			streamId: "s2",
			model: { provider: "smoke-badkey", id: "smoke/bad-1", api: "smoke-bad-api" },
			messages: [],
			options: {},
		});
		check("!command apiKey is a typed rejection", badKey.success === false && /!command/.test(badKey.error), badKey);
		const unknownProv = await core.request({
			type: "provider_stream",
			streamId: "s3",
			model: { provider: "nope", id: "nope/1" },
			messages: [],
			options: {},
		});
		check("unknown provider rejects", unknownProv.success === false && /unknown extension provider/.test(unknownProv.error), unknownProv);

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

		// (j) before_agent_start with systemPromptOptions: prompt-customizer mutates
		// sections.tool_guidance and the sidecar renders it via the upstream prompt
		// builder. Runs in an ISOLATED sidecar instance (phase 2, after the main
		// shutdown): in the main instance claude-rules forces a string in the same
		// run, and forced prompts are opaque upstream (sections would not render).

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

		// (h2) line-length cap: over-long lines are rejected with a parse error
		// and the connection survives (v0 stdin hardening)
		core.sendRaw("a".repeat(1024 * 1024 + 1));
		const tooLong = await core.nextEvent(
			(e) => e.type === "response" && e.command === "parse" && /too long/.test(e.error),
			"too-long line",
			5000,
		).catch(() => null);
		check("over-long line rejected with parse error", tooLong?.success === false, tooLong);
		const alive = await core.request({ type: "user_bash", event: { type: "user_bash", command: "echo alive", excludeFromContext: false, cwd: tmp } });
		check("connection alive after over-long line", alive.success === true && alive.data === null, alive);
		child.stdin.write("a".repeat(1024 * 1024 + 1)); // unterminated: no newline yet
		const untermErr = await core.nextEvent(
			(e) => e.type === "response" && e.command === "parse" && /unterminated/.test(e.error),
			"unterminated too-long line",
			5000,
		).catch(() => null);
		check("unterminated over-long line rejected and reset", untermErr !== null, untermErr);
		const alive2 = await core
			.request({ type: "user_bash", event: { type: "user_bash", command: "echo alive2", excludeFromContext: false, cwd: tmp } }, 5000)
			.catch((e) => ({ success: false, error: e.message }));
		check("commands work after unterminated over-long line", alive2.success === true, alive2);
		const unknown = await core.request({ type: "bogus_command" });
		check("unknown command error", unknown.success === false && /Unknown command: bogus_command/.test(unknown.error), unknown);

		// (i) shutdown: a command queued behind session_shutdown must not be dispatched
		core.send({ type: "session_shutdown", event: { type: "session_shutdown", reason: "quit" } });
		core.send({
			id: "tc-post",
			type: "tool_call",
			event: { type: "tool_call", toolName: "bash", toolCallId: "tc_z", input: { command: "rm -rf /" } },
		});
		await new Promise((r) => setTimeout(r, 800)); // window in which a wrong dispatch would ask the gate
		const postShutdownUi = core.events.find((e) => e.type === "extension_ui_request" && e.method === "select");
		check("no dispatch after session_shutdown", !postShutdownUi, postShutdownUi);
		const code = await Promise.race([core.exited, new Promise((r) => setTimeout(() => r("timeout"), 10000))]);
		check("clean exit 0 after session_shutdown", code === 0, code);

		// (j) phase 2: isolated sidecar, prompt-customizer only — section rendering
		// through the upstream prompt builder (buildSystemPrompt, dist/core/system-prompt.js).
		const agentDir2 = path.join(tmp, "agent2");
		mkdirSync(agentDir2, { recursive: true });
		child2 = spawn(
			process.execPath,
			[ENTRY, "--extension-dir", path.join(EXAMPLES, "prompt-customizer.ts"), "--agent-dir", agentDir2],
			{ cwd: tmp, stdio: ["pipe", "pipe", "pipe"] },
		);
		const stderr2 = [];
		child2.stderr.on("data", (d) => {
			stderr2.push(d.toString());
			process.stderr.write(d);
		});
		const core2 = new FakeCore(child2);
		const init2 = await core2.request({
			type: "init",
			protocolVersion: 1,
			capabilities: { ui: ["select", "confirm", "input", "editor", "notify"], timeoutMs: 5000 },
		});
		check("isolated sidecar init", init2.success === true, init2);
		await core2.nextEvent((e) => e.type === "ready", "ready (isolated)");
		const basSections = await core2.request({
			type: "before_agent_start",
			event: { type: "user", messageType: "user", prompt: "hi" },
			systemPrompt: "BASE PROMPT 2",
			systemPromptOptions: { cwd: tmp, selectedTools: ["read", "bash", "write"], sections: {} },
		});
		check(
			"before_agent_start renders tool_guidance section",
			typeof basSections.data?.systemPrompt === "string" &&
				/Use `read` for file contents/.test(basSections.data.systemPrompt),
			basSections.data,
		);
		// (k) broken stdout pipe (core died): orderly exit, no uncaught crash stack
		child2.stdout.destroy();
		core2.send({ id: "crash-trig", type: "tool_call", event: { type: "tool_call", toolName: "bash", toolCallId: "tc_c", input: { command: "echo boom" } } });
		const code2 = await Promise.race([core2.exited, new Promise((r) => setTimeout(() => r("timeout"), 10000))]);
		const stderr2Text = stderr2.join("");
		check(
			"broken stdout: orderly exit, no crash stack",
			code2 !== "timeout" && /stdout write failed/.test(stderr2Text) && !/UnhandledPromiseRejection|\n\s+at /.test(stderr2Text),
			{ code2, tail: stderr2Text.slice(-400) },
		);

		// (l) 701c: the UNMODIFIED upstream custom-provider-gitlab-duo example
		// loads and registers through the real loader (no streamSimple
		// invocation — it needs a live GitLab endpoint).
		const GLD = path.join(EXAMPLES, "custom-provider-gitlab-duo");
		const agentDir3 = path.join(tmp, "agent3");
		mkdirSync(agentDir3, { recursive: true });
		child3 = spawn(
			process.execPath,
			[ENTRY, "--extension-dir", GLD, "--agent-dir", agentDir3],
			{ cwd: tmp, stdio: ["pipe", "pipe", "pipe"] },
		);
		child3.stderr.on("data", (d) => process.stderr.write(d));
		const core3 = new FakeCore(child3);
		const init3 = await core3.request({
			type: "init",
			protocolVersion: 1,
			capabilities: { ui: [], timeoutMs: 5000 },
		});
		check("gitlab-duo sidecar init", init3.success === true, init3);
		const ready3 = await core3.nextEvent((e) => e.type === "ready", "ready (gitlab-duo)");
		const gld = ready3.providers?.find((p) => p?.name === "gitlab-duo");
		check(
			"gitlab-duo loads unmodified (1 extension, no errors)",
			ready3.extensions.length === 1 && ready3.errors.length === 0,
			ready3.errors,
		);
		check("gitlab-duo provider registered with 9 models", gld && gld.models.length === 9, gld && gld.models?.length);
		check(
			"gitlab-duo model entry shape (provider/id/api)",
			gld?.models?.[0] && gld.models[0].provider === "gitlab-duo" && gld.models[0].id === "claude-opus-4-8" && gld.models[0].api === "gitlab-duo-api",
			gld?.models?.[0],
		);
		core3.send({ type: "session_shutdown", event: { type: "session_shutdown", reason: "quit" } });
		const code3 = await Promise.race([core3.exited, new Promise((r) => setTimeout(() => r("timeout"), 10000))]);
		check("gitlab-duo sidecar clean exit 0", code3 === 0, code3);
	} catch (err) {
		failed++;
		console.error(`FAIL - smoke aborted: ${err.message}`);
		try {
			child.kill("SIGKILL");
			child2?.kill("SIGKILL");
			child3?.kill("SIGKILL");
		} catch {}
	}

	rmSync(tmp, { recursive: true, force: true }); // teardown: remove our temp fixtures

	console.error(`\nsmoke: ${passed} passed, ${failed} failed`);
	process.exit(failed === 0 ? 0 : 1);
}

main();
