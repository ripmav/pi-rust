export default function (pi) {
  pi.registerCommand("dialog-check", { handler: async (_args, ctx) => {
    // Round-trips over the bridge: each answer must come from the TUI user.
    const ok = await ctx.ui.confirm("Proceed?", "E2E confirm dialog");
    const sel = await ctx.ui.select("Pick one", ["alpha", "beta"]);
    const inp = await ctx.ui.input("Name", "type here");
    if (ok !== true || sel !== "beta" || inp !== "amy") {
      throw new Error(`dialog answers wrong: ${JSON.stringify({ ok, sel, inp })}`);
    }
    ctx.ui.notify("dialog done", "info");
    ctx.ui.setStatus("e2e", "status-ok");
    ctx.ui.setTitle("dialog-ok");
  } });
}
