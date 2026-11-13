/**
 * WP-702 e2e fixture: the minimal MCP-registration extension — it only calls
 * `pi.registerMcpServer()` with the local fixture server (no MCP logic).
 *
 * No unmodified upstream MCP-using example exists (upstream
 * packages/coding-agent/examples/extensions contains no registerMcpServer
 * usage), so this fixture pins the extension registration path per the
 * WP-702 acceptance.
 */
import path from "node:path";
import { fileURLToPath } from "node:url";

const fixture = path.join(path.dirname(fileURLToPath(import.meta.url)), "mcp-server-fixture.mjs");

export default function (pi: any) {
	// process.execPath = the node running the sidecar (no PATH dependency).
	pi.registerMcpServer("fixture-reg", { command: process.execPath, args: [fixture], exposure: "direct" });
}
