/**
 * test-only fixture, not an unmodified upstream extension
 *
 * WP-1006 headless contract: without the `custom` capability (headless
 * init advertises ui: []), ctx.ui.custom must resolve with the upstream
 * noOp result (undefined) — no pane events, no hang, no crash. A command
 * (not a tool) because the headless tool path has no ExtensionContext.
 */
export default function (pi) {
	pi.registerCommand("custom-probe", {
		handler: async (_args, ctx) => {
			let factoryCalls = 0;
			const result = await ctx.ui.custom(() => {
				factoryCalls += 1;
				throw new Error("factory must not run without the custom capability");
			});
			if (result !== undefined || factoryCalls !== 0) {
				throw new Error(
					`custom-probe mismatch: result=${String(result)} factoryCalls=${factoryCalls}`,
				);
			}
		},
	});
}
