/**
 * test-only fixture, not an unmodified upstream extension
 *
 * WP-1006 acceptance (WP-707 memo, shape 2): the progress pane — live
 * descriptor updates (step 1/3 -> step 2/3) while the component runs, and
 * a cancel key (esc) that resolves the promise with "cancelled".
 */
export default function (pi) {
	pi.registerCommand("progress-check", {
		handler: async (_args, ctx) => {
			const result = await ctx.ui.custom((tui, theme, _keybindings, done) => {
				let step = 1;
				let finished = false;
				const component = {
					render: (_width) => [theme.fg("muted", `progress: step ${step}/3 ...`)],
					handleInput: (data) => {
						// Esc cancels (the pane's cancel key).
						if (!finished && data === "\u001b") {
							finished = true;
							clearTimeout(timer);
							done("cancelled");
						}
					},
					invalidate: () => {},
					dispose: () => clearTimeout(timer),
				};
				// Live update: the second step pushes a pane_update without
				// any key input (requestRender is the sidecar's relay).
				const timer = setTimeout(() => {
					step = 2;
					tui.requestRender();
				}, 400);
				return component;
			});
			if (result !== "cancelled") {
				throw new Error(`progress answer wrong: ${JSON.stringify(result)}`);
			}
			ctx.ui.notify("progress done", "info");
		},
	});
}
