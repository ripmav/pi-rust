// WP-802: RPC client shim — the upstream RpcClient spawns
// `node <cliPath> --mode rpc ...`; this shim re-execs the pi-rust binary
// with the same args and pipes stdio 1:1 (upstream cli.js would spawn
// `node dist/cli.js --mode rpc ...`).
import { spawn } from "node:child_process";

const bin = process.env.PI_RUST_BIN;
if (!bin) {
	console.error("PI_RUST_BIN is not set");
	process.exit(1);
}
const child = spawn(bin, process.argv.slice(2), { stdio: "inherit" });
child.on("exit", (code, signal) => {
	if (signal) process.kill(process.pid, signal);
	process.exit(code ?? 0);
});
process.on("SIGTERM", () => child.kill("SIGTERM"));
