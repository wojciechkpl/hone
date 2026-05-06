import { For, Show, createSignal, type Component } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { streamRun } from "../lib/sse";
import { baseUrl } from "../lib/api";
import { parseMarkdown, type Node as MdNode } from "../lib/markdown";
import { addUsage } from "../lib/cost";
import type { AgentEvent } from "../types/events";

type Bubble =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string; done: boolean }
  | { kind: "tool"; name: string; status: "running" | "done" | "error"; result?: string }
  | { kind: "error"; message: string };

type ChatPanelProps = {
  onSlashCommand?: (cmd: string, arg: string) => { ok: boolean; reply?: string };
};

const SLASH_COMMANDS = [
  { name: "/theme", desc: "set theme: crimson | chalk | neon | minimal" },
  { name: "/clear", desc: "reset cost meter" },
  { name: "/help", desc: "list available commands" },
];

export const ChatPanel: Component<ChatPanelProps> = (props) => {
  const [bubbles, setBubbles] = createStore<Bubble[]>([]);
  const [draft, setDraft] = createSignal("");
  const [streaming, setStreaming] = createSignal(false);
  let aborter: AbortController | null = null;
  let listEl: HTMLDivElement | undefined;

  function append(bubble: Bubble) {
    setBubbles(bubbles.length, bubble);
    queueMicrotask(() => listEl?.scrollTo({ top: listEl.scrollHeight }));
  }

  function appendToken(text: string) {
    setBubbles(
      produce((arr) => {
        const idx = arr.length - 1;
        if (idx < 0) return;
        const last = arr[idx];
        if (!last || last.kind !== "assistant") return;
        last.text += text;
      }),
    );
    queueMicrotask(() => listEl?.scrollTo({ top: listEl.scrollHeight }));
  }

  function dispatch(event: AgentEvent) {
    switch (event.type) {
      case "token": {
        const idx = bubbles.length - 1;
        if (idx < 0 || bubbles[idx]?.kind !== "assistant") {
          append({ kind: "assistant", text: "", done: false });
        }
        appendToken(event.text);
        break;
      }
      case "tool_call_start": {
        append({ kind: "tool", name: event.name, status: "running" });
        break;
      }
      case "tool_call_end": {
        setBubbles(
          produce((arr) => {
            const idx = arr.length - 1;
            const last = arr[idx];
            if (last?.kind === "tool") {
              last.status = event.is_error ? "error" : "done";
              last.result =
                typeof event.result === "string"
                  ? event.result
                  : JSON.stringify(event.result);
            }
          }),
        );
        break;
      }
      case "done": {
        setBubbles(
          produce((arr) => {
            const idx = arr.length - 1;
            if (idx >= 0 && arr[idx]?.kind === "assistant") {
              (arr[idx] as { kind: "assistant"; text: string; done: boolean }).done = true;
            }
          }),
        );
        addUsage(event.usage, event.cost_usd);
        setStreaming(false);
        break;
      }
      case "error": {
        append({ kind: "error", message: event.message });
        setStreaming(false);
        break;
      }
      // Other variants (worker_*, plan_*, approval_required, etc.) are
      // intentionally ignored in Phase 0 — they're surfaced once the
      // matching panes land.
      default:
        break;
    }
  }

  function send() {
    const prompt = draft().trim();
    if (!prompt || streaming()) return;

    // Slash commands are intercepted client-side; they never go to /run.
    if (prompt.startsWith("/") && props.onSlashCommand) {
      const space = prompt.indexOf(" ");
      const cmd = space < 0 ? prompt : prompt.slice(0, space);
      const arg = space < 0 ? "" : prompt.slice(space + 1).trim();
      const result = props.onSlashCommand(cmd, arg);
      setDraft("");
      append({
        kind: "error",
        message: result.reply ?? (result.ok ? "ok" : "command failed"),
      });
      return;
    }

    setDraft("");
    append({ kind: "user", text: prompt });
    setStreaming(true);

    aborter = streamRun(
      baseUrl,
      null,
      { prompt },
      {
        onEvent: dispatch,
        onError: (e) => append({ kind: "error", message: e }),
        onClose: () => setStreaming(false),
      },
    );
  }

  // Slash autocomplete: open when the draft starts with `/` and has no
  // space yet. Matches against the static SLASH_COMMANDS list.
  function paletteEntries(): typeof SLASH_COMMANDS {
    const d = draft();
    if (!d.startsWith("/") || d.includes(" ")) return [];
    return SLASH_COMMANDS.filter((c) => c.name.startsWith(d));
  }

  function cancel() {
    aborter?.abort();
    aborter = null;
    setStreaming(false);
    append({ kind: "error", message: "cancelled" });
  }

  function onKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      send();
    }
  }

  return (
    <main
      style={{
        "grid-area": "main",
        display: "grid",
        "grid-template-rows": "1fr auto",
        "min-height": 0,
        background: "var(--surface-base)",
      }}
    >
      <div
        ref={listEl}
        class="px-4 py-4"
        style={{
          "overflow-y": "auto",
          display: "flex",
          "flex-direction": "column",
          gap: "12px",
        }}
      >
        <For each={bubbles}>
          {(b) => <BubbleView bubble={b} />}
        </For>
        <Show when={bubbles.length === 0}>
          <div style={{ color: "var(--text-secondary)", "font-size": "0.9em" }}>
            Start a turn — Cmd+Enter to send, type below.
          </div>
        </Show>
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
        class="px-3 py-3 border-t flex flex-col gap-2"
        style={{
          "border-color": "var(--surface-border)",
          background: "var(--surface-raised)",
          position: "relative",
        }}
      >
        <Show when={paletteEntries().length > 0}>
          <ul
            role="listbox"
            style={{
              position: "absolute",
              left: "12px",
              right: "12px",
              bottom: "calc(100% + 4px)",
              "list-style": "none",
              padding: "4px 0",
              margin: 0,
              background: "var(--surface-overlay)",
              border: "1px solid var(--surface-border)",
              "border-radius": "8px",
              "max-height": "200px",
              "overflow-y": "auto",
              "z-index": 20,
            }}
          >
            <For each={paletteEntries()}>
              {(entry) => (
                <li
                  role="option"
                  onClick={() => {
                    setDraft(entry.name + " ");
                  }}
                  style={{
                    padding: "6px 12px",
                    cursor: "pointer",
                    "font-family": "var(--font-mono)",
                    "font-size": "0.85em",
                  }}
                >
                  <strong style={{ color: "var(--accent-hot)" }}>{entry.name}</strong>
                  <span style={{ color: "var(--text-secondary)", "margin-left": "8px" }}>
                    {entry.desc}
                  </span>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <textarea
          rows={3}
          placeholder="Ask Hone…  (Cmd+Enter to send · /help)"
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={onKeydown}
          style={{ resize: "vertical" }}
        />
        <div class="flex justify-end gap-2">
          <Show when={streaming()}>
            <button type="button" onClick={cancel}>
              Cancel
            </button>
          </Show>
          <button type="submit" disabled={streaming() || draft().trim() === ""}>
            Send
          </button>
        </div>
      </form>
    </main>
  );
};

const ToolCardView: Component<{
  card: { kind: "tool"; name: string; status: "running" | "done" | "error"; result?: string };
}> = (props) => {
  // Long results collapse by default; the user can expand inline.
  const [expanded, setExpanded] = createSignal(false);
  const COLLAPSED_LINES = 6;

  const formatted = (): string => {
    const r = props.card.result;
    if (!r) return "";
    // Try to pretty-print JSON; fall back to the raw string on any
    // parse failure (most tool results are already strings).
    try {
      return JSON.stringify(JSON.parse(r), null, 2);
    } catch {
      return r;
    }
  };

  const lines = (): string[] => formatted().split("\n");
  const isLong = (): boolean => lines().length > COLLAPSED_LINES;
  const visible = (): string => {
    if (expanded() || !isLong()) return formatted();
    return lines().slice(0, COLLAPSED_LINES).join("\n");
  };

  return (
    <div
      class="self-start px-3 py-2"
      style={{
        background: "var(--surface-overlay)",
        border:
          props.card.status === "error"
            ? "1px solid var(--role-error)"
            : "1px solid var(--surface-border)",
        "border-radius": "8px",
        "max-width": "720px",
        "font-family": "var(--font-mono)",
        "font-size": "0.85em",
      }}
    >
      <div class="flex items-center" style={{ gap: "8px" }}>
        <strong style={{ color: "var(--role-tool)" }}>{props.card.name}</strong>
        <span style={{ color: "var(--text-secondary)" }}>{props.card.status}</span>
        <Show when={isLong()}>
          <button
            type="button"
            onClick={() => setExpanded(!expanded())}
            style={{
              "margin-left": "auto",
              padding: "2px 8px",
              "font-size": "0.85em",
            }}
          >
            {expanded() ? "collapse" : `show all (${lines().length} lines)`}
          </button>
        </Show>
      </div>
      <Show when={props.card.result}>
        <pre
          style={{
            "white-space": "pre-wrap",
            "max-height": expanded() ? "20em" : `${COLLAPSED_LINES * 1.4}em`,
            "overflow-y": "auto",
            margin: "6px 0 0",
            color: props.card.status === "error" ? "var(--role-error)" : "var(--text-primary)",
          }}
        >
          {visible()}
        </pre>
      </Show>
    </div>
  );
};

const MarkdownView: Component<{ text: string }> = (props) => {
  return (
    <div style={{ "white-space": "pre-wrap", "line-height": "1.55" }}>
      <For each={parseMarkdown(props.text)}>
        {(node) => <MdNodeView node={node} />}
      </For>
    </div>
  );
};

const MdNodeView: Component<{ node: MdNode }> = (props) => {
  const n = props.node;
  switch (n.kind) {
    case "text":
      return <>{n.value}</>;
    case "bold":
      return <strong>{n.value}</strong>;
    case "italic":
      return <em>{n.value}</em>;
    case "code":
      return (
        <code
          style={{
            background: "var(--surface-overlay)",
            border: "1px solid var(--surface-border)",
            "border-radius": "4px",
            padding: "0 4px",
            "font-family": "var(--font-mono)",
            "font-size": "0.9em",
          }}
        >
          {n.value}
        </code>
      );
    case "block":
      return (
        <pre
          style={{
            background: "var(--surface-overlay)",
            border: "1px solid var(--surface-border)",
            "border-radius": "6px",
            padding: "8px 10px",
            margin: "8px 0",
            "font-family": "var(--font-mono)",
            "font-size": "0.85em",
            "white-space": "pre",
            "overflow-x": "auto",
          }}
        >
          <Show when={n.lang}>
            <span
              style={{
                display: "block",
                color: "var(--text-secondary)",
                "font-size": "0.7em",
                "text-transform": "uppercase",
                "letter-spacing": "0.05em",
                "margin-bottom": "4px",
              }}
            >
              {n.lang}
            </span>
          </Show>
          {n.value}
        </pre>
      );
  }
};

const BubbleView: Component<{ bubble: Bubble }> = (props) => {
  return (
    <Show when={props.bubble}>
      {(b) => {
        const x = b();
        switch (x.kind) {
          case "user":
            return (
              <div
                class="self-end px-3 py-2"
                style={{
                  background: "var(--role-user)",
                  color: "var(--surface-base)",
                  "border-radius": "10px",
                  "max-width": "720px",
                  "white-space": "pre-wrap",
                }}
              >
                {x.text}
              </div>
            );
          case "assistant":
            return (
              <div
                class="self-start px-3 py-2"
                style={{
                  background: "var(--surface-raised)",
                  border: "1px solid var(--surface-border)",
                  "border-left": "3px solid var(--role-assistant)",
                  "border-radius": "10px",
                  "max-width": "720px",
                }}
              >
                <MarkdownView text={x.text} />
                <Show when={!x.done}>
                  <span style={{ color: "var(--accent-hot)" }}>▍</span>
                </Show>
              </div>
            );
          case "tool":
            return <ToolCardView card={x} />;
          case "error":
            return (
              <div
                class="self-start px-3 py-2"
                style={{
                  background: "transparent",
                  border: "1px solid var(--role-error)",
                  color: "var(--role-error)",
                  "border-radius": "8px",
                  "max-width": "720px",
                  "font-family": "var(--font-mono)",
                  "font-size": "0.85em",
                }}
              >
                {x.message}
              </div>
            );
        }
      }}
    </Show>
  );
};
