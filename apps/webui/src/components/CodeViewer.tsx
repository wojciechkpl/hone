import {
  Show,
  createResource,
  type Component,
} from "solid-js";
import { fetchText } from "../lib/api";
import { highlightCode, langFromPath } from "../lib/highlight";

type Props = {
  /** Path relative to workspace_root. */
  path: string | null;
};

type LoadedFile = {
  text: string;
  /** Highlighted HTML if Shiki returned a result, else null. */
  html: string | null;
  lang: string;
};

async function fetchFileContent(path: string | null): Promise<LoadedFile | null> {
  if (!path) return null;
  const text = await fetchText(`/files/content?path=${encodeURIComponent(path)}`);
  const lang = langFromPath(path);
  // Trigger Shiki lazy-load; nullable so the fallback stays in sync.
  const html = await highlightCode(text, lang);
  return { text, html, lang };
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
            <Show when={content()}>
              <CodeBody file={content()!} />
            </Show>
          </Show>
        </Show>
      </Show>
    </div>
  );
};

const CodeBody: Component<{ file: LoadedFile }> = (props) => {
  return (
    <Show
      when={props.file.html}
      fallback={<CodeWithLineNumbers text={props.file.text} />}
    >
      <ShikiBlock html={props.file.html!} />
    </Show>
  );
};

/** Render Shiki's pre-built HTML. Shiki returns `<pre><code>...</code></pre>`
 * with inline styles per token, so we just inject it. The container
 * styling comes from the parent <pre> defaults. */
const ShikiBlock: Component<{ html: string }> = (props) => {
  return (
    <div
      style={{ "min-width": "100%" }}
      // eslint-disable-next-line solid/no-innerhtml
      innerHTML={props.html}
    />
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
