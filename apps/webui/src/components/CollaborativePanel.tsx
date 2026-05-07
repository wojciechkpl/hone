import {
  For,
  Show,
  createEffect,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import { getJson, postJson, type Participant } from "../lib/api";
import { useActiveSession } from "../lib/session";

const NAME_KEY = "hone-webui:collab-name";
const USER_KEY = "hone-webui:collab-user-id";

function loadName(): string {
  try {
    return localStorage.getItem(NAME_KEY) ?? "";
  } catch {
    return "";
  }
}

function loadUserId(): string {
  try {
    let id = localStorage.getItem(USER_KEY);
    if (!id) {
      id =
        typeof crypto !== "undefined" && crypto.randomUUID
          ? crypto.randomUUID()
          : `u-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
      localStorage.setItem(USER_KEY, id);
    }
    return id;
  } catch {
    return `u-${Date.now()}`;
  }
}

export const CollaborativePanel: Component = () => {
  const sessionId = useActiveSession();
  const [open, setOpen] = createSignal(false);
  const [version, setVersion] = createSignal(0);
  const [name, setName] = createSignal(loadName());
  const userId = loadUserId();
  const [error, setError] = createSignal<string | null>(null);

  const [participants] = createResource(
    () => ({ id: sessionId(), v: version() }),
    async ({ id }) => {
      if (!id) return [];
      try {
        return await getJson<Participant[]>(
          `/sessions/${encodeURIComponent(id)}/participants`,
          null,
        );
      } catch {
        return [];
      }
    },
  );

  // Persist the name on every keystroke.
  createEffect(() => {
    try {
      localStorage.setItem(NAME_KEY, name());
    } catch {
      /* ignore */
    }
  });

  function joined(): Participant | null {
    const list = participants();
    if (!list) return null;
    return list.find((p) => p.user_id === userId) ?? null;
  }

  function isOwner(): boolean {
    return joined()?.role === "Owner";
  }

  async function join() {
    const id = sessionId();
    if (!id) return;
    const trimmed = name().trim();
    if (!trimmed) {
      setError("name is required");
      return;
    }
    setError(null);
    try {
      const resp = await postJson(
        `/sessions/${encodeURIComponent(id)}/join`,
        { user_id: userId, name: trimmed },
        null,
      );
      if (!resp.ok) throw new Error(`join ${resp.status}`);
      setVersion(version() + 1);
    } catch (e) {
      setError(String(e));
    }
  }

  async function leave() {
    const id = sessionId();
    if (!id) return;
    try {
      const resp = await postJson(
        `/sessions/${encodeURIComponent(id)}/leave`,
        { user_id: userId },
        null,
      );
      if (!resp.ok && resp.status !== 204) throw new Error(`leave ${resp.status}`);
      setVersion(version() + 1);
    } catch (e) {
      setError(String(e));
    }
  }

  async function kick(targetUserId: string) {
    const id = sessionId();
    if (!id) return;
    if (!confirm("Kick this participant?")) return;
    try {
      const resp = await postJson(
        `/sessions/${encodeURIComponent(id)}/kick`,
        { owner_user_id: userId, target_user_id: targetUserId },
        null,
      );
      if (!resp.ok && resp.status !== 204) throw new Error(`kick ${resp.status}`);
      setVersion(version() + 1);
    } catch (e) {
      setError(String(e));
    }
  }

  function badge(): string {
    const list = participants();
    if (!list || list.length === 0) return "collab: 0";
    return `collab: ${list.length}`;
  }

  return (
    <div style={{ position: "relative" }}>
      <button
        class="px-2 py-1"
        style={{ "font-size": "0.85em" }}
        onClick={() => setOpen(!open())}
        aria-haspopup="dialog"
        aria-expanded={open()}
        title="View / join the participant list for this session"
      >
        {badge()} ▾
      </button>
      <Show when={open()}>
        <div
          role="dialog"
          aria-label="Collaborative session"
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 4px)",
            "min-width": "320px",
            "max-width": "400px",
            background: "var(--surface-raised)",
            border: "1px solid var(--surface-border)",
            "border-radius": "8px",
            "box-shadow": "0 12px 40px rgba(0,0,0,0.4)",
            "z-index": 50,
            padding: "12px",
            display: "flex",
            "flex-direction": "column",
            gap: "10px",
          }}
        >
          <Show
            when={sessionId()}
            fallback={
              <p style={{ margin: 0, color: "var(--text-secondary)", "font-size": "0.85em" }}>
                Pick a session before joining.
              </p>
            }
          >
            <Show
              when={joined()}
              fallback={
                <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
                  <label
                    style={{
                      "font-size": "0.75em",
                      color: "var(--text-secondary)",
                      display: "grid",
                      gap: "4px",
                    }}
                  >
                    Display name
                    <input
                      type="text"
                      value={name()}
                      onInput={(e) => setName(e.currentTarget.value)}
                      placeholder="alice"
                    />
                  </label>
                  <button onClick={join} style={{ "align-self": "flex-start" }}>
                    Join session
                  </button>
                </div>
              }
            >
              <div
                style={{
                  "font-size": "0.8em",
                  color: "var(--text-secondary)",
                  display: "flex",
                  "align-items": "center",
                  gap: "8px",
                }}
              >
                <span>
                  joined as <strong style={{ color: "var(--accent-hot)" }}>{joined()!.name}</strong>{" "}
                  ({joined()!.role.toLowerCase()})
                </span>
                <button
                  onClick={leave}
                  style={{
                    "margin-left": "auto",
                    padding: "2px 8px",
                    "font-size": "0.85em",
                  }}
                >
                  Leave
                </button>
              </div>
            </Show>

            <div>
              <strong
                style={{
                  "font-size": "0.75em",
                  "text-transform": "uppercase",
                  "letter-spacing": "0.05em",
                  color: "var(--text-secondary)",
                }}
              >
                Participants
              </strong>
              <Show
                when={participants() && participants()!.length > 0}
                fallback={
                  <p
                    style={{
                      margin: "6px 0 0",
                      color: "var(--text-secondary)",
                      "font-size": "0.8em",
                    }}
                  >
                    no one in this session yet
                  </p>
                }
              >
                <ul
                  style={{
                    "list-style": "none",
                    margin: "6px 0 0",
                    padding: 0,
                    display: "flex",
                    "flex-direction": "column",
                    gap: "4px",
                  }}
                >
                  <For each={participants()!}>
                    {(p) => (
                      <li
                        style={{
                          display: "grid",
                          "grid-template-columns": "1fr auto",
                          "align-items": "center",
                          gap: "8px",
                          padding: "4px 6px",
                          background: "var(--surface-overlay)",
                          border: "1px solid var(--surface-border)",
                          "border-radius": "6px",
                          "font-size": "0.85em",
                        }}
                      >
                        <span>
                          <strong style={{ "font-family": "var(--font-mono)" }}>{p.name}</strong>
                          <span style={{ color: "var(--text-secondary)", "margin-left": "6px" }}>
                            {p.role.toLowerCase()}
                          </span>
                        </span>
                        <Show when={isOwner() && p.user_id !== userId}>
                          <button
                            onClick={() => kick(p.user_id)}
                            title="Kick this participant"
                            style={{
                              padding: "2px 8px",
                              "font-size": "0.75em",
                              "border-color": "var(--role-error)",
                              color: "var(--role-error)",
                            }}
                          >
                            Kick
                          </button>
                        </Show>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </div>
          </Show>

          <Show when={error()}>
            <p style={{ "font-size": "0.75em", color: "var(--role-error)", margin: 0 }}>
              {error()}
            </p>
          </Show>
        </div>
      </Show>
    </div>
  );
};
