import type { Component } from "solid-js";

export const StatusBar: Component = () => {
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
      <span>Hone web v0.1.0 · Phase 0 scaffold</span>
      <span>SolidJS · UnoCSS · Vite</span>
    </footer>
  );
};
