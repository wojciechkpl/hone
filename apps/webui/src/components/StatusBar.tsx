import type { Component } from "solid-js";
import { useCost } from "../lib/cost";

export const StatusBar: Component = () => {
  const cost = useCost();
  return (
    <footer
      class="px-4 flex items-center justify-between border-t"
      style={{
        "border-color": "var(--surface-border)",
        background: "var(--surface-raised)",
        color: "var(--text-secondary)",
        "font-size": "0.8em",
        height: "28px",
        "grid-area": "statusbar",
        "font-family": "var(--font-mono, ui-monospace, monospace)",
      }}
    >
      <span>Hone web v0.1.1 · Phase 1</span>
      <span title="Tokens / cost in this session">
        {cost().tokens.toLocaleString()} tok · ${cost().usd.toFixed(4)}
      </span>
    </footer>
  );
};
