import type { Component } from "solid-js";
import { AgentSelector } from "./AgentSelector";
import { SessionSelector } from "./SessionSelector";
import { CollaborativePanel } from "./CollaborativePanel";
import { useStepMode, setStepMode } from "../lib/step";
import { togglePreview, usePreviewOpen } from "../lib/preview";
import { toggleSettings } from "../lib/settings";

export type Theme = "crimson" | "chalk" | "neon" | "minimal";

type Props = {
  theme: Theme;
  onCycleTheme: () => void;
  healthy: boolean | null;
  onRefreshHealth: () => void;
  onSwitchSession: (id: string) => void;
};

export const TopBar: Component<Props> = (props) => {
  const stepMode = useStepMode();
  const previewOpen = usePreviewOpen();
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
        <SessionSelector onSwitch={props.onSwitchSession} />
      </div>

      <div class="flex items-center gap-3" style={{ color: "var(--text-secondary)" }}>
        <CollaborativePanel />
        <AgentSelector />
        <label
          class="px-2 py-1 flex items-center"
          style={{
            gap: "6px",
            "font-size": "0.85em",
            cursor: "pointer",
            border: "1px solid var(--surface-border)",
            "border-radius": "6px",
          }}
          title="Pause for risky tools (write/shell). When off, all tools auto-approve."
        >
          <input
            type="checkbox"
            checked={stepMode()}
            onInput={(e) => setStepMode(e.currentTarget.checked)}
          />
          step
        </label>
        <button
          class="px-2 py-1"
          style={{ "font-size": "0.85em" }}
          onClick={props.onRefreshHealth}
          title="Click to re-probe daemon /health"
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
          onClick={togglePreview}
          title="Toggle file preview pane"
        >
          {previewOpen() ? "preview ▾" : "preview ▸"}
        </button>
        <button
          class="px-2 py-1"
          style={{ "font-size": "0.85em" }}
          onClick={props.onCycleTheme}
          title="Cycle theme (crimson → chalk → neon → minimal)"
        >
          theme: {props.theme}
        </button>
        <button
          class="px-2 py-1"
          style={{ "font-size": "0.85em" }}
          onClick={toggleSettings}
          aria-label="Open settings"
          title="Open settings"
        >
          ⚙
        </button>
      </div>
    </header>
  );
};
