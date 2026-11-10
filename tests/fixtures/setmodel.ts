export default function (pi) {
  let count = 0;
  const t0 = Date.now();
  const mark = (m) => console.error(`[fx +${Date.now() - t0}ms] ${m}`);
  pi.on("before_agent_start", async () => {
    count += 1;
    mark(`before_agent_start #${count}`);
    if (count !== 2) return;
    const bad = await pi.setModel({ id: "nope-model", provider: "openai" });
    mark(`bad=${bad}`);
    if (bad) throw new Error("invalid setModel was accepted");
    const good = await pi.setModel({ id: "gpt-4o-mini", provider: "openai" });
    mark(`good=${good}`);
    if (!good) throw new Error("valid setModel was declined");
  });
}
