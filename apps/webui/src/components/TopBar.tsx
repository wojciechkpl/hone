import type { Component } from "solid-js";

type Props = {
  theme: "crimson" | "chalk";
  onToggleTheme: () => void;
  healthy: boolean | null;
  onRefreshHealth: () => void;
};

export const TopBar: Component<Props> = (props) => {
  return (
    <header
      class="px-4 flex items-center justify-between border-b"
      style={{
        "border-color": "var(--surface-border)",
        background: "var(--surface-raised)",
        height: "48px",
        "grid-area": "topbar",
      }}
    >
      <div class="flex items-center gap-3">
        <strong style={{ "letter-spacing": "0.05em" }}>HONE</strong>
        <span style={{ color: "var(--text-secondary)", "font-size": "0.85em" }}>web</span>
      </div>

      <div class="flex items-center gap-3" style={{ color: "var(--text-secondary)" }}>
        <button
          class="px-2 py-1"
          style={{ "font-size": "0.85em" }}
          onClick={props.onRefreshHealth}
        >
          <span
            class="dot"
            classList={{
              "dot--ok": props.healthy === true,
              "dot--bad": props.healthy === false,
              "dot--unknown": props.healthy === null,
            }}
            style={{ "margin-right": "6px", "vertical-align": "middle" }}
          />
          {props.healthy === true ? "daemon ok" : props.healthy === false ? "daemon down" : "checking…"}
        </button>
        <button
          class="px-2 py-1"
          style={{ "font-size": "0.85em" }}
          onClick={props.onToggleTheme}
          title="Toggle theme"
        >
          theme: {props.theme}
        </button>
      </div>
    </header>
  );
};
