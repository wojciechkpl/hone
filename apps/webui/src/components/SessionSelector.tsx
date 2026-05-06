import {
  For,
  Show,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { listSessions, deleteSession, type SessionMeta } from "../lib/db";
import {
  useActiveSession,
  setActiveSession,
  ensureSession,
} from "../lib/session";

type Props = {
  /** Called after the user picks (or creates) a different session. The
   * parent reloads messages for the new id from IDB. */
  onSwitch?: (id: string) => void;
};

export const SessionSelector: Component<Props> = (props) => {
  const [open, setOpen] = createSignal(false);
  const [version, setVersion] = createSignal(0);
  const active = useActiveSession();

  const [sessions] = createResource(version, async () => {
    const all = await listSessions();
    return all.sort((a: SessionMeta, b: SessionMeta) => b.last_used_at - a.last_used_at);
  });

  async function newSession() {
    const id = await ensureSession(null);
    setVersion(version() + 1);
    setOpen(false);
    props.onSwitch?.(id);
  }

  function pick(id: string) {
    setActiveSession(id);
    setOpen(false);
    props.onSwitch?.(id);
  }

  async function remove(id: string, e: Event) {
    e.stopPropagation();
    if (!confirm("Delete this session?")) return;
    await deleteSession(id);
    if (active() === id) {
      setActiveSession(null);
      props.onSwitch?.("");
    }
    setVersion(version() + 1);
  }

  function label(): string {
    const id = active();
    if (!id) return "(no session)";
    const all = sessions();
    const meta = all?.find((s) => s.id === id);
    return meta?.title?.slice(0, 28) ?? `${id.slice(0, 8)}…`;
  }

  return (
    <div style={{ position: "relative" }}>
      <button
        class="px-2 py-1"
        style={{ "font-size": "0.85em", "max-width": "240px", overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}
        onClick={() => setOpen(!open())}
        aria-haspopup="listbox"
        aria-expanded={open()}
      >
        session: {label()} ▾
      </button>
      <Show when={open()}>
        <div
          role="listbox"
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 4px)",
            "min-width": "320px",
            "max-height": "360px",
            "overflow-y": "auto",
            background: "var(--surface-raised)",
            border: "1px solid var(--surface-border)",
            "border-radius": "8px",
            "box-shadow": "0 12px 40px rgba(0,0,0,0.4)",
            "z-index": 50,
          }}
        >
          <button
            onClick={newSession}
            style={{
              display: "block",
              width: "100%",
              "text-align": "left",
              padding: "8px 10px",
              background: "transparent",
              border: "none",
              "border-bottom": "1px solid var(--surface-border)",
              "border-radius": 0,
              cursor: "pointer",
              color: "var(--accent-hot)",
              "font-weight": 600,
            }}
          >
            + new session
          </button>
          <Show
            when={!sessions.loading}
            fallback={<div style={{ padding: "10px", color: "var(--text-secondary)" }}>loading…</div>}
          >
            <Show
              when={sessions() && sessions()!.length > 0}
              fallback={<div style={{ padding: "10px", color: "var(--text-secondary)" }}>no sessions yet</div>}
            >
              <For each={sessions()!}>
                {(s) => (
                  <div
                    role="option"
                    aria-selected={active() === s.id}
                    onClick={() => pick(s.id)}
                    style={{
                      display: "grid",
                      "grid-template-columns": "1fr auto",
                      "align-items": "center",
                      gap: "6px",
                      padding: "8px 10px",
                      background:
                        active() === s.id
                          ? "var(--surface-overlay)"
                          : "transparent",
                      "border-bottom": "1px solid var(--surface-border)",
                      cursor: "pointer",
                      color: "var(--text-primary)",
                    }}
                  >
                    <div>
                      <div style={{ "font-family": "var(--font-mono)", "font-size": "0.8em" }}>
                        {s.title ?? s.id.slice(0, 12) + "…"}
                      </div>
                      <div
                        style={{
                          color: "var(--text-secondary)",
                          "font-size": "0.7em",
                          "margin-top": "2px",
                        }}
                      >
                        {new Date(s.last_used_at).toLocaleString()}
                      </div>
                    </div>
                    <button
                      onClick={(e) => remove(s.id, e)}
                      title="Delete session"
                      style={{
                        padding: "2px 8px",
                        "font-size": "0.85em",
                        background: "transparent",
                        border: "1px solid var(--surface-border)",
                        color: "var(--text-secondary)",
                      }}
                    >
                      ✕
                    </button>
                  </div>
                )}
              </For>
            </Show>
          </Show>
        </div>
      </Show>
    </div>
  );
};
