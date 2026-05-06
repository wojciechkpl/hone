import { For, Show, createSignal, type Component } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { streamRun } from "../lib/sse";
import { baseUrl } from "../lib/api";
import type { AgentEvent } from "../types/events";

type Bubble =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string; done: boolean }
  | { kind: "tool"; name: string; status: "running" | "done" | "error"; result?: string }
  | { kind: "error"; message: string };

export const ChatPanel: Component = () => {
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
        style={{ "border-color": "var(--surface-border)", background: "var(--surface-raised)" }}
      >
        <textarea
          rows={3}
          placeholder="Ask Hone…  (Cmd+Enter to send)"
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
                  "white-space": "pre-wrap",
                }}
              >
                {x.text}
                <Show when={!x.done}>
                  <span style={{ color: "var(--accent-hot)" }}>▍</span>
                </Show>
              </div>
            );
          case "tool":
            return (
              <div
                class="self-start px-3 py-2"
                style={{
                  background: "var(--surface-overlay)",
                  border:
                    x.status === "error"
                      ? "1px solid var(--role-error)"
                      : "1px solid var(--surface-border)",
                  "border-radius": "8px",
                  "max-width": "720px",
                  "font-family": "var(--font-mono)",
                  "font-size": "0.85em",
                }}
              >
                <strong style={{ color: "var(--role-tool)" }}>{x.name}</strong>
                <span style={{ color: "var(--text-secondary)", "margin-left": "8px" }}>
                  {x.status}
                </span>
                <Show when={x.result}>
                  <pre style={{ "white-space": "pre-wrap", "max-height": "8em", "overflow-y": "auto", margin: "6px 0 0" }}>
                    {x.result}
                  </pre>
                </Show>
              </div>
            );
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
