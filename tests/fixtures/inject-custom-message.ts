/**
 * Regression e2e fixture: injects a deja-style custom message via
 * before_agent_start — string content, no role, the exact
 * `Pick<CustomMessage>` shape the upstream runner passes to the core.
 */
export default function (pi: any) {
	pi.on("before_agent_start", () => ({
		message: {
			customType: "e2e-note",
			content: "CUSTOM-NOTE-42: remember the number 42",
			display: false,
		},
	}));
}
