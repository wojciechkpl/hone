import { For, Show, createEffect, createResource, createSignal, type Component } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { streamRun, type RunRequest } from "../lib/sse";
import { baseUrl } from "../lib/api";
import { parseMarkdown, type Node as MdNode } from "../lib/markdown";
import { addUsage } from "../lib/cost";
import { ensureSession, touchSession, useActiveSession } from "../lib/session";
import { loadMessages, saveMessages, type PersistedMessage } from "../lib/db";
import { setPendingApproval, useStepMode } from "../lib/step";
import { openFile } from "../lib/preview";
import { looksLikeDiff } from "../lib/diff";
import { highlightCode } from "../lib/highlight";
import {
  completeWorker,
  resetOrchestratorState,
  setBudget,
  setPlan,
  startWorker,
} from "../lib/orchestrator";
import { SideBySideDiff } from "./SideBySideDiff";
import { WorkerProgressPanel } from "./WorkerProgressPanel";
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
  { name: "/settings", desc: "open the settings drawer" },
  { name: "/clear", desc: "reset cost meter" },
  { name: "/help", desc: "list available commands" },
];

/** Tools that carry a path arg we can auto-open in the preview pane. */
const FILE_TOOLS = new Set(["read_file", "edit_file", "write_file"]);

/**
 * Tool args sometimes include the daemon's absolute path
 * (`/Users/.../proj/foo.rs`). The /files endpoints accept relative
 * paths only — strip a common workspace prefix when we can guess one
 * from the leading slash. The daemon sets `workspace_root` to its CWD
 * by default; the GUI doesn't know what that is, so we just trim a
 * leading `/` and let the endpoint reject if it escapes.
 */
function stripWorkspacePrefix(path: string): string {
  // If path is absolute, the simplest fallback is the basename — the
  // file tree shows everything under workspace_root anyway, so this
  // succeeds when the file exists at the root and gracefully 404s
  // otherwise. Future improvement: GET /workspace would let us
  // compute the correct relative path here.
  if (path.startsWith("/")) {
    const segs = path.split("/").filter(Boolean);
    return segs[segs.length - 1] ?? "";
  }
  return path.replace(/^\.\//, "");
}

export const ChatPanel: Component<ChatPanelProps> = (props) => {
  const [bubbles, setBubbles] = createStore<Bubble[]>([]);
  const [draft, setDraft] = createSignal("");
  const [streaming, setStreaming] = createSignal(false);
  let aborter: AbortController | null = null;
  let listEl: HTMLDivElement | undefined;
  const activeSession = useActiveSession();
  const stepMode = useStepMode();
  // The id we're streaming under right now (so step events route back).
  let activeRunId: string | null = null;

  // Hydrate from IDB whenever the active session changes. Bubbles are
  // a flat replay of what was persisted — close to the original render
  // but without live SSE state (token cursor, etc.).
  createEffect(async () => {
    const id = activeSession();
    setBubbles([]);
    if (!id) return;
    const msgs = await loadMessages(id);
    setBubbles(msgs.map(persistedToBubble));
    queueMicrotask(() => listEl?.scrollTo({ top: listEl.scrollHeight }));
  });

  /** Persist the current bubble list under the active session. */
  async function persist() {
    const id = activeSession();
    if (!id) return;
    const ts = Date.now();
    const msgs: PersistedMessage[] = bubbles.map((b) => bubbleToPersisted(b, ts));
    await saveMessages(id, msgs);
  }

  function append(bubble: Bubble) {
    setBubbles(bubbles.length, bubble);
    queueMicrotask(() => listEl?.scrollTo({ top: listEl.scrollHeight }));
    void persist();
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
    // Don't persist on every token — too chatty. We persist on
    // milestone events (tool start/end, done, error) below.
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
        // Auto-open the file in the preview pane for any tool that
        // names a path. read_file / edit_file / write_file all carry
        // either `path` or `file_path`. Best-effort: if there's no
        // path arg (shell, glob, grep) we leave the pane untouched.
        if (FILE_TOOLS.has(event.name)) {
          const args = (event.args ?? {}) as Record<string, unknown>;
          const path = (args["path"] ?? args["file_path"]) as unknown;
          if (typeof path === "string" && path.length > 0) {
            openFile(stripWorkspacePrefix(path));
          }
        }
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
        void persist();
        break;
      }
      case "tool_call_paused": {
        // Step-mode: the agent paused before a risky tool — surface the
        // approval modal. The active run id was generated client-side
        // when we POSTed /run with an AskRisky policy.
        if (activeRunId) {
          setPendingApproval({
            runId: activeRunId,
            toolId: event.id,
            name: event.name,
            args: event.args,
          });
        }
        break;
      }
      case "plan_created": {
        setPlan({ goal: event.goal, subtask_count: event.subtask_count });
        break;
      }
      case "worker_started": {
        startWorker({
          task_id: event.task_id,
          agent: event.agent,
          description: event.description,
        });
        break;
      }
      case "worker_completed": {
        completeWorker(
          event.task_id,
          event.status === "completed" ? "completed" : "failed",
          event.cost_usd,
        );
        break;
      }
      case "budget_progress": {
        setBudget({
          spent_usd: event.spent_usd,
          budget_usd: event.budget_usd,
          workers_completed: event.workers_completed,
          workers_total: event.workers_total,
        });
        break;
      }
      case "approval_required": {
        // Generic approval (rare today; emitted by some agents at
        // session level rather than per tool call). Show a modal anyway
        // — Skip / Deny still work.
        if (activeRunId) {
          setPendingApproval({
            runId: activeRunId,
            toolId: `req-${Date.now()}`,
            name: event.action || "approval_required",
            args: event.description,
          });
        }
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
        void persist();
        break;
      }
      case "error": {
        append({ kind: "error", message: event.message });
        setStreaming(false);
        break;
      }
      // Other variants (worker_*, plan_*, memory_recalled) are
      // intentionally ignored in Phase 1B — they're surfaced once the
      // matching panes land.
      default:
        break;
    }
  }

  async function send() {
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
    // Each new turn starts with a clean orchestrator slate.
    resetOrchestratorState();
    append({ kind: "user", text: prompt });
    setStreaming(true);

    // Make sure we have a session and bump its title/last_used.
    const id = await ensureSession(activeSession());
    void touchSession(id, prompt.slice(0, 60));

    // Mint a stable run id so the daemon's run-registry and our
    // approval modal share the same correlation key.
    const runId =
      typeof crypto !== "undefined" && crypto.randomUUID
        ? crypto.randomUUID()
        : `r-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
    activeRunId = runId;

    const body: RunRequest = {
      prompt,
      request_id: runId,
    };
    if (stepMode()) {
      // Honed wants the policy as a serde tag union. AskRisky pauses
      // for write/shell tools but auto-allows read-only ones.
      // The generic `RunRequest.approval_policy` field accepts any
      // serializable shape; we cast through unknown to avoid widening
      // the typed wire shape just for one optional field.
      (body as unknown as { approval_policy: unknown }).approval_policy = {
        AskRisky: { always_allowed: [] },
      };
    }

    aborter = streamRun(
      baseUrl,
      null,
      body,
      {
        onEvent: dispatch,
        onError: (e) => append({ kind: "error", message: e }),
        onClose: () => {
          setStreaming(false);
          activeRunId = null;
        },
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
    <section
      style={{
        display: "grid",
        "grid-template-rows": "auto 1fr auto",
        "min-height": 0,
        background: "var(--surface-base)",
      }}
    >
      <WorkerProgressPanel />
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
    </section>
  );
};

function bubbleToPersisted(b: Bubble, ts: number): PersistedMessage {
  switch (b.kind) {
    case "user":
      return { kind: "user", text: b.text, ts };
    case "assistant":
      return { kind: "assistant", text: b.text, ts };
    case "tool":
      return {
        kind: "tool",
        name: b.name,
        status: b.status,
        result: b.result,
        ts,
      };
    case "error":
      return { kind: "error", message: b.message, ts };
  }
}

function persistedToBubble(m: PersistedMessage): Bubble {
  switch (m.kind) {
    case "user":
      return { kind: "user", text: m.text };
    case "assistant":
      return { kind: "assistant", text: m.text, done: true };
    case "tool":
      return {
        kind: "tool",
        name: m.name,
        status: m.status,
        result: m.result,
      };
    case "error":
      return { kind: "error", message: m.message };
  }
}

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
        <Show
          when={looksLikeDiff(props.card.result ?? "")}
          fallback={
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
          }
        >
          <div
            style={{
              "max-height": expanded() ? "30em" : `${COLLAPSED_LINES * 2}em`,
              "overflow-y": "auto",
              "margin-top": "6px",
            }}
          >
            <SideBySideDiff text={props.card.result ?? ""} />
          </div>
        </Show>
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
      return <HighlightedBlock lang={n.lang} value={n.value} />;
  }
};

const HighlightedBlock: Component<{ lang: string; value: string }> = (props) => {
  const [html] = createResource(
    () => ({ lang: props.lang, value: props.value }),
    async (input) => (input.lang ? highlightCode(input.value, input.lang) : null),
  );
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
      <Show when={props.lang}>
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
          {props.lang}
        </span>
      </Show>
      <Show
        when={!html.loading && html()}
        fallback={<>{props.value}</>}
      >
        <span
          // eslint-disable-next-line solid/no-innerhtml
          innerHTML={html()!}
        />
      </Show>
    </pre>
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
