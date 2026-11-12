// WP-608 e2e fixture: a registered command that echoes its args. The TUI
// slash menu shows it (extension source); invoking it runs the M1 bridge
// `command` path and the result renders in the transcript.
export default function (pi) {
  pi.registerCommand("echo-cmd", {
    description: "Echo the args (e2e)",
    handler: async (args) => `echo: ${args}`,
  });
}
