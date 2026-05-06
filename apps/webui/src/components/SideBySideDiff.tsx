import { For, Show, type Component } from "solid-js";
import { pairLines, parseUnifiedDiff, type SideBySideRow } from "../lib/diff";

type Props = {
  /** Raw unified-diff text, typically from a tool result. */
  text: string;
};

export const SideBySideDiff: Component<Props> = (props) => {
  const files = () => parseUnifiedDiff(props.text);
  return (
    <div
      style={{
        "font-family": "var(--font-mono)",
        "font-size": "0.8em",
        "border-radius": "6px",
        overflow: "hidden",
        border: "1px solid var(--surface-border)",
        background: "var(--surface-base)",
      }}
    >
      <For each={files()}>{(f) => <FileBlock file={f} />}</For>
    </div>
  );
};

const FileBlock: Component<{
  file: ReturnType<typeof parseUnifiedDiff>[number];
}> = (props) => {
  const path = () => props.file.newPath ?? props.file.oldPath ?? "(unnamed)";
  return (
    <div>
      <Show when={props.file.oldPath || props.file.newPath}>
        <div
          style={{
            padding: "4px 8px",
            background: "var(--surface-overlay)",
            "border-bottom": "1px solid var(--surface-border)",
            color: "var(--text-secondary)",
            "font-size": "0.85em",
          }}
        >
          {path()}
        </div>
      </Show>
      <For each={props.file.hunks}>
        {(hunk) => (
          <div>
            <div
              style={{
                padding: "2px 8px",
                background: "var(--diff-hunk-bg)",
                color: "var(--diff-hunk-fg)",
                "font-size": "0.85em",
              }}
            >
              {hunk.header}
            </div>
            <table
              style={{
                width: "100%",
                "border-collapse": "collapse",
                "table-layout": "fixed",
              }}
            >
              <colgroup>
                <col style={{ width: "50%" }} />
                <col style={{ width: "50%" }} />
              </colgroup>
              <For each={pairLines(hunk)}>{(row) => <Row row={row} />}</For>
            </table>
          </div>
        )}
      </For>
    </div>
  );
};

const Row: Component<{ row: SideBySideRow }> = (props) => {
  const r = props.row;
  switch (r.kind) {
    case "context":
      return (
        <tr>
          <td style={cellStyle(undefined)}>{r.left}</td>
          <td style={cellStyle(undefined)}>{r.right}</td>
        </tr>
      );
    case "removed":
      return (
        <tr>
          <td style={cellStyle("remove")}>{r.left}</td>
          <td style={cellStyle("empty")} />
        </tr>
      );
    case "added":
      return (
        <tr>
          <td style={cellStyle("empty")} />
          <td style={cellStyle("add")}>{r.right}</td>
        </tr>
      );
    case "modified":
      return (
        <tr>
          <td style={cellStyle("remove")}>{r.left}</td>
          <td style={cellStyle("add")}>{r.right}</td>
        </tr>
      );
  }
};

function cellStyle(kind: "add" | "remove" | "empty" | undefined): Record<string, string> {
  const base: Record<string, string> = {
    padding: "1px 8px",
    "white-space": "pre-wrap",
    "word-break": "break-word",
    "vertical-align": "top",
    "border-bottom": "1px solid var(--surface-border)",
  };
  if (kind === "add") {
    base.background = "var(--diff-add-bg)";
    base.color = "var(--diff-add-fg)";
  } else if (kind === "remove") {
    base.background = "var(--diff-remove-bg)";
    base.color = "var(--diff-remove-fg)";
  } else if (kind === "empty") {
    base.background = "var(--surface-overlay)";
  }
  return base;
}
