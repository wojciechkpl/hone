import {
  Show,
  createResource,
  type Component,
} from "solid-js";
import { fetchText } from "../lib/api";

type Props = {
  /** Path relative to workspace_root. */
  path: string | null;
};

async function fetchFileContent(path: string | null): Promise<string | null> {
  if (!path) return null;
  return fetchText(`/files/content?path=${encodeURIComponent(path)}`);
}

export const CodeViewer: Component<Props> = (props) => {
  const [content] = createResource(() => props.path, fetchFileContent);

  return (
    <div
      style={{
        flex: 1,
        "min-height": 0,
        overflow: "auto",
        background: "var(--surface-base)",
        "font-family": "var(--font-mono)",
        "font-size": "0.85em",
      }}
    >
      <Show
        when={props.path}
        fallback={
          <div style={{ padding: "12px", color: "var(--text-secondary)" }}>
            Click a file in the tree, or run a tool that touches a file —
            the preview opens automatically.
          </div>
        }
      >
        <Show
          when={!content.loading}
          fallback={<div style={{ padding: "12px", color: "var(--text-secondary)" }}>loading…</div>}
        >
          <Show
            when={!content.error}
            fallback={
              <div style={{ padding: "12px", color: "var(--role-error)" }}>
                {String(content.error)}
              </div>
            }
          >
            <Show when={content() !== null && content() !== undefined}>
              <CodeWithLineNumbers text={content() ?? ""} />
            </Show>
          </Show>
        </Show>
      </Show>
    </div>
  );
};

const CodeWithLineNumbers: Component<{ text: string }> = (props) => {
  const lines = (): string[] => props.text.split("\n");
  const totalDigits = (): number => Math.max(2, String(lines().length).length);

  return (
    <pre
      style={{
        margin: 0,
        padding: "8px 0",
        "white-space": "pre",
        // Theme-driven baseline; future Phase 2B swaps in Shiki.
        color: "var(--text-primary)",
      }}
    >
      {lines().map((line, i) => (
        <div
          style={{
            display: "grid",
            "grid-template-columns": `${totalDigits()}ch 1fr`,
            "column-gap": "12px",
            padding: "0 12px",
          }}
        >
          <span
            style={{
              color: "var(--text-muted)",
              "text-align": "right",
              "user-select": "none",
            }}
          >
            {i + 1}
          </span>
          <span>{line || " "}</span>
        </div>
      ))}
    </pre>
  );
};
