// WP-1102 e2e fixture: injects a sentinel into the system prompt on every
// before_agent_start. Placed under <cwd>/.pi/extensions/ it must only take
// effect when the project is trusted (project-extension trust gate).
export default function (pi) {
  pi.on("before_agent_start", async (event) => ({
    systemPrompt: (event.systemPrompt ?? "") + "\nTRUST-MARKER-SENTINEL",
  }));
}
