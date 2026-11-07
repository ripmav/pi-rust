#!/usr/bin/env node
/**
 * pi-extension-sidecar — runs the original upstream pi extension loader and
 * ExtensionRunner, serving the bridge protocol (NDJSON) on stdio.
 *
 * Usage: pi-extension-sidecar [--extension-dir <path>]... [--agent-dir <path>]
 * All logs go to stderr; stdout is protocol output only.
 */
import { runBridge } from "./bridge.mjs";

const args = process.argv.slice(2);
const extensionDirs = [];
let agentDir;
for (let i = 0; i < args.length; i++) {
	const arg = args[i];
	if (arg === "--extension-dir") {
		const value = args[++i];
		if (!value) {
			console.error("error: --extension-dir requires a path");
			process.exit(2);
		}
		extensionDirs.push(value);
	} else if (arg === "--agent-dir") {
		const value = args[++i];
		if (!value) {
			console.error("error: --agent-dir requires a path");
			process.exit(2);
		}
		agentDir = value;
	} else if (arg === "--help" || arg === "-h") {
		console.error("usage: pi-extension-sidecar [--extension-dir <path>]... [--agent-dir <path>]");
		process.exit(0);
	} else {
		console.error(`error: unknown argument: ${arg}`);
		process.exit(2);
	}
}

try {
	await runBridge({ extensionDirs, agentDir });
} catch (err) {
	console.error(`[sidecar] fatal: ${err instanceof Error ? (err.stack ?? err.message) : err}`);
	process.exit(1);
}
