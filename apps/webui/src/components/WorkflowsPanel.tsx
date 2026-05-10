import {
  For,
  Show,
  createMemo,
  createResource,
  createSignal,
  type Component,
} from "solid-js";
import {
  getJson,
  postJson,
  type RecipeDetail,
  type RecipeSummary,
} from "../lib/api";
import {
  extractTemplateVars,
  setWorkflowsOpen,
  substituteVars,
  useWorkflowsOpen,
} from "../lib/workflows";

type Props = {
  /**
   * Called when the user clicks Run on a workflow. Receives the fully
   * substituted prompt — the parent (App.tsx / ChatPanel) is responsible
   * for actually dispatching it through the chat input.
   */
  onRun: (prompt: string) => void;
};

export const WorkflowsPanel: Component<Props> = (props) => {
  const open = useWorkflowsOpen();
  const [refresh, setRefresh] = createSignal(0);
  const [selected, setSelected] = createSignal<string | null>(null);
  const [vars, setVars] = createSignal<Record<string, string>>({});
  const [createOpen, setCreateOpen] = createSignal(false);

  const [recipes] = createResource(
    () => ({ open: open(), v: refresh() }),
    async ({ open: o }) => {
      if (!o) return [];
      try {
        return await getJson<RecipeSummary[]>("/recipes", null);
      } catch {
        return [];
      }
    },
  );

  const [detail] = createResource(
    () => ({ name: selected(), open: open() }),
    async ({ name, open: o }) => {
      if (!o || !name) return null;
      try {
        return await getJson<RecipeDetail>(
          `/recipes/${encodeURIComponent(name)}`,
          null,
        );
      } catch {
        return null;
      }
    },
  );

  const templateVars = createMemo(() => {
    const d = detail();
    if (!d) return [];
    return extractTemplateVars(d.steps);
  });

  function close() {
    setWorkflowsOpen(false);
    setSelected(null);
    setCreateOpen(false);
  }

  function runSelected() {
    const d = detail();
    if (!d) return;
    const v = vars();
    // Compose a single prompt from each step's substituted text. The
    // backend doesn't currently accept multi-step plans through /run, so
    // we concatenate with section headers and let the agent treat it as
    // one turn. Future: a /recipes/{name}/run endpoint that does the
    // step-by-step orchestration server-side.
    const composed = d.steps
      .map((s) => `## ${s.name}\n${substituteVars(s.prompt, v)}`)
      .join("\n\n");
    props.onRun(composed);
    close();
  }

  return (
    <Show when={open()}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Workflows"
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(0,0,0,0.4)",
          "z-index": 200,
          display: "flex",
          "justify-content": "flex-end",
        }}
        onClick={(e) => {
          if (e.target === e.currentTarget) close();
        }}
      >
        <aside
          style={{
            width: "min(560px, 96vw)",
            height: "100%",
            background: "var(--surface-raised)",
            "border-left": "1px solid var(--surface-border)",
            "box-shadow": "-12px 0 40px rgba(0,0,0,0.4)",
            display: "flex",
            "flex-direction": "column",
          }}
        >
          <header
            class="px-4"
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              "border-bottom": "1px solid var(--surface-border)",
              height: "48px",
            }}
          >
            <strong>Workflows</strong>
            <div style={{ display: "flex", gap: "6px" }}>
              <button
                onClick={() => setCreateOpen(!createOpen())}
                style={{ padding: "4px 10px", "font-size": "0.85em" }}
              >
                {createOpen() ? "Cancel new" : "+ New"}
              </button>
              <button
                onClick={close}
                style={{ padding: "4px 10px", "font-size": "0.85em" }}
              >
                ✕
              </button>
            </div>
          </header>

          <div
            style={{
              flex: 1,
              "overflow-y": "auto",
              padding: "16px",
              display: "flex",
              "flex-direction": "column",
              gap: "12px",
            }}
          >
            <Show when={createOpen()}>
              <CreateForm onCreated={() => { setCreateOpen(false); setRefresh(refresh() + 1); }} />
            </Show>

            <Show
              when={!recipes.loading}
              fallback={<p style={{ color: "var(--text-secondary)" }}>loading…</p>}
            >
              <Show
                when={recipes() && recipes()!.length > 0}
                fallback={
                  <p style={{ color: "var(--text-secondary)", "font-size": "0.85em" }}>
                    No workflows found. Drop a YAML file under
                    <code> ~/.config/hone/recipes/</code> or click <em>+ New</em>.
                  </p>
                }
              >
                <div
                  style={{
                    display: "grid",
                    "grid-template-columns": "1fr",
                    gap: "8px",
                  }}
                >
                  <For each={recipes()!}>
                    {(r) => (
                      <button
                        onClick={() => { setSelected(r.name); setVars({}); }}
                        aria-pressed={selected() === r.name}
                        style={{
                          "text-align": "left",
                          padding: "10px 12px",
                          "border-color":
                            selected() === r.name
                              ? "var(--accent-hot)"
                              : "var(--surface-border)",
                          background:
                            selected() === r.name
                              ? "var(--surface-overlay)"
                              : "var(--surface-base)",
                          display: "flex",
                          "flex-direction": "column",
                          gap: "2px",
                        }}
                      >
                        <div
                          style={{ display: "flex", "align-items": "baseline", gap: "8px" }}
                        >
                          <strong style={{ "font-family": "var(--font-mono)" }}>{r.name}</strong>
                          <span
                            style={{
                              color: "var(--text-secondary)",
                              "font-size": "0.75em",
                            }}
                          >
                            {r.source} · {r.step_count} step{r.step_count === 1 ? "" : "s"}
                          </span>
                        </div>
                        <Show when={r.description}>
                          <span
                            style={{
                              color: "var(--text-secondary)",
                              "font-size": "0.85em",
                            }}
                          >
                            {r.description}
                          </span>
                        </Show>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </Show>

            <Show when={detail()}>
              <section
                style={{
                  "border-top": "1px solid var(--surface-border)",
                  "padding-top": "12px",
                  display: "flex",
                  "flex-direction": "column",
                  gap: "8px",
                }}
              >
                <Show when={templateVars().length > 0}>
                  <strong
                    style={{
                      "font-size": "0.75em",
                      "text-transform": "uppercase",
                      "letter-spacing": "0.05em",
                      color: "var(--text-secondary)",
                    }}
                  >
                    Variables
                  </strong>
                  <For each={templateVars()}>
                    {(name) => (
                      <label
                        style={{
                          display: "grid",
                          "grid-template-columns": "120px 1fr",
                          "align-items": "center",
                          gap: "8px",
                          "font-size": "0.85em",
                        }}
                      >
                        <span style={{ "font-family": "var(--font-mono)" }}>{name}</span>
                        <input
                          type="text"
                          value={vars()[name] ?? ""}
                          onInput={(e) =>
                            setVars({ ...vars(), [name]: e.currentTarget.value })
                          }
                          placeholder={`value for ${name}`}
                        />
                      </label>
                    )}
                  </For>
                </Show>

                <details>
                  <summary style={{ cursor: "pointer", "font-size": "0.85em" }}>
                    Steps
                  </summary>
                  <ol
                    style={{
                      margin: "8px 0 0",
                      padding: "0 0 0 18px",
                      display: "flex",
                      "flex-direction": "column",
                      gap: "4px",
                      "font-size": "0.8em",
                    }}
                  >
                    <For each={detail()!.steps}>
                      {(s) => (
                        <li>
                          <strong>{s.name}</strong>
                          <pre
                            style={{
                              margin: "2px 0 0",
                              padding: "4px 8px",
                              background: "var(--surface-overlay)",
                              "border-radius": "4px",
                              "white-space": "pre-wrap",
                            }}
                          >
                            {s.prompt}
                          </pre>
                        </li>
                      )}
                    </For>
                  </ol>
                </details>

                <button onClick={runSelected} style={{ "align-self": "flex-start" }}>
                  Run workflow
                </button>
              </section>
            </Show>
          </div>
        </aside>
      </div>
    </Show>
  );
};

const CreateForm: Component<{ onCreated: () => void }> = (props) => {
  const [name, setName] = createSignal("");
  const [description, setDescription] = createSignal("");
  const [stepName, setStepName] = createSignal("step1");
  const [stepPrompt, setStepPrompt] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  async function submit() {
    setError(null);
    if (!name().trim() || !stepName().trim() || !stepPrompt().trim()) {
      setError("name, step name, and step prompt are all required");
      return;
    }
    setBusy(true);
    try {
      const resp = await postJson(
        "/recipes",
        {
          name: name().trim(),
          description: description().trim() || null,
          steps: [{ name: stepName().trim(), prompt: stepPrompt().trim() }],
        },
        null,
      );
      if (!resp.ok) {
        const text = await resp.text().catch(() => "");
        setError(`create failed: ${resp.status} ${text}`);
        return;
      }
      props.onCreated();
    } catch (e) {
      setError(`create failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      style={{
        display: "flex",
        "flex-direction": "column",
        gap: "8px",
        padding: "12px",
        background: "var(--surface-overlay)",
        border: "1px solid var(--surface-border)",
        "border-radius": "6px",
      }}
    >
      <strong style={{ "font-size": "0.85em" }}>New workflow</strong>
      <p style={{ margin: 0, color: "var(--text-secondary)", "font-size": "0.75em" }}>
        Saved to <code>~/.config/hone/recipes/&lt;name&gt;.yaml</code>. Use{" "}
        <code>{"{{var}}"}</code> in the prompt for runtime template vars.
      </p>
      <input
        type="text"
        placeholder="name (a-z, 0-9, _, -)"
        value={name()}
        onInput={(e) => setName(e.currentTarget.value)}
      />
      <input
        type="text"
        placeholder="description (optional)"
        value={description()}
        onInput={(e) => setDescription(e.currentTarget.value)}
      />
      <input
        type="text"
        placeholder="first step name"
        value={stepName()}
        onInput={(e) => setStepName(e.currentTarget.value)}
      />
      <textarea
        placeholder="first step prompt — use {{var}} for template vars"
        value={stepPrompt()}
        onInput={(e) => setStepPrompt(e.currentTarget.value)}
        rows="4"
        style={{ "font-family": "var(--font-mono)", "font-size": "0.85em" }}
      />
      <Show when={error()}>
        <p style={{ "font-size": "0.75em", color: "var(--role-error)", margin: 0 }}>
          {error()}
        </p>
      </Show>
      <button onClick={submit} disabled={busy()} style={{ "align-self": "flex-start" }}>
        {busy() ? "Saving…" : "Save"}
      </button>
    </section>
  );
};
