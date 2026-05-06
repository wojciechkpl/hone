import {
  For,
  Show,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { getJson, type AgentSummary } from "../lib/api";
import { useActiveAgent, setActiveAgent } from "../lib/agent";

async function fetchAgents(): Promise<AgentSummary[]> {
  return getJson<AgentSummary[]>("/agents", null);
}

export const AgentSelector: Component = () => {
  const [open, setOpen] = createSignal(false);
  const [agents] = createResource(fetchAgents);
  const active = useActiveAgent();

  return (
    <div style={{ position: "relative" }}>
      <button
        class="px-2 py-1"
        style={{ "font-size": "0.85em" }}
        onClick={() => setOpen(!open())}
        aria-haspopup="listbox"
        aria-expanded={open()}
      >
        agent: {active()?.name ?? "(default)"} ▾
      </button>
      <Show when={open()}>
        <div
          role="listbox"
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 4px)",
            "min-width": "260px",
            "max-height": "320px",
            "overflow-y": "auto",
            background: "var(--surface-raised)",
            border: "1px solid var(--surface-border)",
            "border-radius": "8px",
            "box-shadow": "0 12px 40px rgba(0,0,0,0.4)",
            "z-index": 50,
          }}
        >
          <Show when={!agents.loading} fallback={<div style={{ padding: "10px", color: "var(--text-secondary)" }}>loading…</div>}>
            <Show when={agents() && agents()!.length > 0} fallback={<div style={{ padding: "10px", color: "var(--text-secondary)" }}>no agents</div>}>
              <For each={agents()!}>
                {(a) => (
                  <button
                    role="option"
                    aria-selected={active()?.name === a.name}
                    onClick={() => {
                      setActiveAgent(a);
                      setOpen(false);
                    }}
                    style={{
                      display: "block",
                      width: "100%",
                      "text-align": "left",
                      padding: "8px 10px",
                      background:
                        active()?.name === a.name
                          ? "var(--surface-overlay)"
                          : "transparent",
                      border: "none",
                      "border-bottom": "1px solid var(--surface-border)",
                      "border-radius": 0,
                      cursor: "pointer",
                      color: "var(--text-primary)",
                    }}
                  >
                    <div style={{ "font-family": "var(--font-mono)", "font-weight": 600 }}>
                      {a.name}
                      <span
                        style={{
                          "margin-left": "6px",
                          "font-size": "0.75em",
                          color: "var(--text-secondary)",
                          "font-weight": 400,
                        }}
                      >
                        {a.source}
                      </span>
                    </div>
                    <div
                      style={{
                        "font-size": "0.75em",
                        color: "var(--text-secondary)",
                        "margin-top": "2px",
                      }}
                    >
                      {a.description}
                    </div>
                  </button>
                )}
              </For>
            </Show>
          </Show>
        </div>
      </Show>
    </div>
  );
};
