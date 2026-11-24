/**
 * WP-702 e2e fixture: the minimal MCP-registration extension — it only calls
 * `pi.registerMcpServer()` with the local fixture server (no MCP logic).
 *
 * No unmodified upstream MCP-using example exists (upstream
 * packages/coding-agent/examples/extensions contains no registerMcpServer
 * usage), so this fixture pins the extension registration path per the
 * WP-702 acceptance.
 *
 * E3S5/WP-1405 + follow-up (D13, PO 2026-11-24): the sidecar hosts no MCP
 * servers any more — the registration is accepted by the unmodified runner
 * and FORWARDED to the core over the bridge (mcp_server_register): the
 * core hosts the server itself (spawned exactly once, its tools declared
 * and callable natively, in the mcp_servers_change full set — asserted by
 * the e2e wp1405_register_mcp_server_not_hosted_pin). The e2e passes
 * `MCP_REGISTER_SPAWN_LOG` to count process starts.
 */
import path from "node:path";
import { fileURLToPath } from "node:url";

const fixture = path.join(path.dirname(fileURLToPath(import.meta.url)), "mcp-server-fixture.mjs");

export default function (pi: any) {
	// process.execPath = the node running the sidecar (no PATH dependency).
	const args = [fixture];
	const spawnLog = process.env.MCP_REGISTER_SPAWN_LOG;
	if (spawnLog) args.push("--spawn-log", spawnLog);
	pi.registerMcpServer("fixture-reg", { command: process.execPath, args, exposure: "direct" });
}
