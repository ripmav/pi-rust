/**
 * test-only fixture, not an unmodified upstream extension
 *
 * WP-1006 acceptance (WP-707 memo, shape 1): the mcp-manager menu — a
 * SelectList inside a framed container, driven by cursor + confirm keys.
 * The pane must render in the TUI, receive the keys, and resolve with the
 * selected value; anything but "beta" fails the command (extension_error).
 */
import { Container, SelectList, Text } from "@earendil-works/pi-tui";

// The bridge identity theme only has fg/strikethrough; SelectList's render
// needs its own theme shape (passthrough keeps the pane text plain).
const selectTheme = {
	fg: (_style, text) => text,
	bold: (text) => text,
	selectedText: (text) => text,
	description: (text) => text,
	noMatch: (text) => text,
	scrollInfo: (text) => text,
};

export default function (pi) {
	pi.registerCommand("menu-check", {
		handler: async (_args, ctx) => {
			const result = await ctx.ui.custom((_tui, theme, _keybindings, done) => {
				const items = [
					{ value: "alpha", description: "first" },
					{ value: "beta", description: "second" },
					{ value: "gamma", description: "third" },
				];
				const list = new SelectList(items, 3, selectTheme);
				list.onSelect = (item) => done(item.value);
				list.onCancel = () => done(undefined);
				const container = new Container();
				container.addChild(new Text(theme.fg("accent", "Menu check"), 1, 0));
				container.addChild(list);
				return container;
			});
			// The sidecar process is shared with progress-check: it reads the
			// answer back through the progress pane's live line — the only
			// surface visible while the next pane is open (a pane replaces the
			// transcript area, so notifies are hidden then).
			globalThis.__wp1006Menu = result;
			if (result !== "beta") {
				throw new Error(`menu answer wrong: ${JSON.stringify(result)}`);
			}
			ctx.ui.notify("menu done", "info");
		},
	});
}
