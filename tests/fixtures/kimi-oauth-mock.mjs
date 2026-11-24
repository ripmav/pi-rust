#!/usr/bin/env node
// WP-1007/E3S4 test fixture: a Kimi Code OAuth mock.
// KIMI_CODE_OAUTH_HOST points at this server:
//   POST /api/oauth/device_authorization (form: client_id)
//     -> device code response (E3S4: the core-native login flow)
//   POST /api/oauth/token
//     (form: client_id, grant_type=refresh_token, refresh_token — refresh
//      rotated tokens; or client_id, device_code, grant_type=device_code —
//      login polling, completes on the first poll)
// Each request is appended as JSON to the log file.
import http from "node:http";
import fs from "node:fs";

const logFile = process.argv[2];
if (!logFile) {
	console.error("usage: kimi-oauth-mock.mjs <refresh-log-file>");
	process.exit(2);
}

const server = http.createServer((req, res) => {
	let body = "";
	req.on("data", (c) => (body += c));
	req.on("end", () => {
		if (req.method === "POST" && req.url === "/api/oauth/device_authorization") {
			const port = server.address().port;
			fs.appendFileSync(logFile, JSON.stringify({ url: req.url, body }) + "\n");
			res.writeHead(200, { "Content-Type": "application/json" });
			res.end(
				JSON.stringify({
					device_code: "mock-device-code",
					user_code: "KIMI-MOCK",
					verification_uri: `http://127.0.0.1:${port}/activate`,
					verification_uri_complete: `http://127.0.0.1:${port}/activate?user_code=KIMI-MOCK`,
					interval: 1,
					expires_in: 60,
				}),
			);
			return;
		}
		if (req.method === "POST" && req.url === "/api/oauth/token") {
			fs.appendFileSync(logFile, JSON.stringify({ url: req.url, body }) + "\n");
			res.writeHead(200, { "Content-Type": "application/json" });
			res.end(
				JSON.stringify({
					access_token: "new-access",
					refresh_token: "new-refresh",
					expires_in: 3600,
				}),
			);
			return;
		}
		res.writeHead(404);
		res.end();
	});
});
server.listen(0, "127.0.0.1", () => {
	console.log(`READY ${server.address().port}`);
});
