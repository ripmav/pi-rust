/**
 * WP-705 e2e fixture: registers one extension shortcut that resolves
 * cleanly (ctrl+shift+x, not a built-in) and one that conflicts with a
 * RESERVED built-in (ctrl+c = app.clear). Test-only fixture, not an
 * unmodified upstream extension.
 */
export default function (pi) {
	pi.registerShortcut("ctrl+shift+x", {
		description: "wp705 fixture action",
		handler: async () => {
			pi.appendEntry("wp705-fired", {});
		},
	});
	pi.registerShortcut("ctrl+c", {
		description: "reserved conflict probe",
		handler: async () => {},
	});
}
