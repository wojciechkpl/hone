import { type Component } from "solid-js";
import { FileTreeBrowser } from "./FileTreeBrowser";
import { CodeViewer } from "./CodeViewer";
import { useActiveFile, setPreviewVisible } from "../lib/preview";

export const FilePreviewPanel: Component = () => {
  const active = useActiveFile();

  return (
    <aside
      style={{
        display: "grid",
        "grid-template-rows": "auto 1fr",
        "min-height": 0,
        "border-left": "1px solid var(--surface-border)",
        background: "var(--surface-raised)",
      }}
    >
      <div
        class="px-3 py-2 border-b flex items-center justify-between"
        style={{
          "border-color": "var(--surface-border)",
          color: "var(--text-secondary)",
          "font-size": "0.85em",
        }}
      >
        <span style={{ "font-family": "var(--font-mono)" }}>
          {active() ?? "(no file)"}
        </span>
        <button
          onClick={() => setPreviewVisible(false)}
          style={{ padding: "2px 8px", "font-size": "0.85em" }}
          title="Close preview pane"
        >
          ✕
        </button>
      </div>
      <div
        style={{
          display: "grid",
          "grid-template-columns": "200px 1fr",
          "min-height": 0,
        }}
      >
        <div
          style={{
            "border-right": "1px solid var(--surface-border)",
            "overflow-y": "auto",
            "min-height": 0,
            background: "var(--surface-base)",
            padding: "6px 0",
          }}
        >
          <FileTreeBrowser path="" />
        </div>
        <CodeViewer path={active()} />
      </div>
    </aside>
  );
};
