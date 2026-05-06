import {
  For,
  Show,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { getJson, type FileEntry } from "../lib/api";
import { openFile, useActiveFile } from "../lib/preview";

type Props = {
  /** Directory path relative to workspace_root. Empty string = root. */
  path: string;
  depth?: number;
};

async function fetchListing(path: string): Promise<FileEntry[]> {
  const url = path ? `/files?path=${encodeURIComponent(path)}` : "/files";
  return getJson<FileEntry[]>(url, null);
}

export const FileTreeBrowser: Component<Props> = (props) => {
  const depth = props.depth ?? 0;
  const [listing] = createResource(() => props.path, fetchListing);
  return (
    <Show
      when={!listing.loading}
      fallback={
        <div
          style={{
            "padding-left": `${depth * 12 + 12}px`,
            color: "var(--text-secondary)",
            "font-size": "0.8em",
          }}
        >
          loading…
        </div>
      }
    >
      <Show
        when={listing()}
        fallback={
          <div
            style={{
              "padding-left": `${depth * 12 + 12}px`,
              color: "var(--role-error)",
              "font-size": "0.8em",
            }}
          >
            failed to list
          </div>
        }
      >
        <For each={listing()!}>
          {(entry) => <FileTreeNode entry={entry} depth={depth} />}
        </For>
      </Show>
    </Show>
  );
};

const FileTreeNode: Component<{ entry: FileEntry; depth: number }> = (props) => {
  const [expanded, setExpanded] = createSignal(false);
  const active = useActiveFile();

  function onClick() {
    if (props.entry.is_dir) {
      setExpanded(!expanded());
    } else {
      openFile(props.entry.path);
    }
  }

  return (
    <div>
      <button
        onClick={onClick}
        style={{
          display: "block",
          width: "100%",
          "text-align": "left",
          background:
            !props.entry.is_dir && active() === props.entry.path
              ? "var(--surface-overlay)"
              : "transparent",
          color:
            !props.entry.is_dir && active() === props.entry.path
              ? "var(--accent-hot)"
              : "var(--text-primary)",
          border: "none",
          "border-radius": "4px",
          padding: `2px ${props.depth * 12 + 8}px`,
          "padding-left": `${props.depth * 12 + 8}px`,
          "font-family": "var(--font-mono)",
          "font-size": "0.8em",
          cursor: "pointer",
        }}
      >
        <span style={{ "margin-right": "4px", color: "var(--text-secondary)" }}>
          {props.entry.is_dir ? (expanded() ? "▾" : "▸") : "·"}
        </span>
        {props.entry.name}
      </button>
      <Show when={props.entry.is_dir && expanded()}>
        <FileTreeBrowser path={props.entry.path} depth={props.depth + 1} />
      </Show>
    </div>
  );
};
