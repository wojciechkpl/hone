import { For, Show, type Component } from "solid-js";
import {
  clearDryRunPreview,
  setPendingPlanAction,
  useBudget,
  useDryRunPreview,
  usePlan,
  useWorkers,
  type DryRunPreview,
  type Worker,
} from "../lib/orchestrator";

export const WorkerProgressPanel: Component = () => {
  const plan = usePlan();
  const workers = useWorkers();
  const budget = useBudget();
  const dryRun = useDryRunPreview();

  // Hide entirely when nothing is in flight.
  return (
    <Show when={plan() || workers().length > 0 || dryRun()}>
      <Show when={dryRun()}>
        <DryRunCard preview={dryRun()!} />
      </Show>
      <Show when={!dryRun() && (plan() || workers().length > 0)}>
      <div
        style={{
          margin: "0 12px 12px",
          padding: "10px 12px",
          background: "var(--surface-overlay)",
          border: "1px solid var(--surface-border)",
          "border-radius": "8px",
        }}
      >
        <Show when={plan()}>
          <div
            style={{
              "font-size": "0.8em",
              color: "var(--text-secondary)",
              "margin-bottom": "6px",
            }}
          >
            <strong style={{ color: "var(--accent-hot)" }}>plan</strong>
            <span style={{ "margin-left": "8px" }}>{plan()!.goal}</span>
            <span style={{ "margin-left": "8px" }}>
              ({plan()!.subtask_count} subtask{plan()!.subtask_count === 1 ? "" : "s"})
            </span>
          </div>
        </Show>
        <Show when={workers().length > 0}>
          <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
            <For each={workers()}>{(w) => <WorkerRow worker={w} />}</For>
          </div>
        </Show>
        <Show when={budget()}>
          <div
            style={{
              "margin-top": "6px",
              "font-family": "var(--font-mono)",
              "font-size": "0.75em",
              color: "var(--text-secondary)",
            }}
          >
            spent ${budget()!.spent_usd.toFixed(4)}
            <Show when={budget()!.budget_usd != null}>
              {" "}
              / ${budget()!.budget_usd!.toFixed(2)}
            </Show>{" "}
            · {budget()!.workers_completed}/{budget()!.workers_total} workers
          </div>
        </Show>
      </div>
      </Show>
    </Show>
  );
};

const DryRunCard: Component<{ preview: DryRunPreview }> = (props) => {
  const p = props.preview;

  function onRun() {
    setPendingPlanAction({ kind: "run", preview: p });
    clearDryRunPreview();
  }
  function onEdit() {
    setPendingPlanAction({ kind: "edit", preview: p });
    clearDryRunPreview();
  }
  function onCancel() {
    setPendingPlanAction({ kind: "cancel" });
    clearDryRunPreview();
  }

  return (
    <div
      role="dialog"
      aria-label="Plan preview"
      style={{
        margin: "0 12px 12px",
        padding: "12px",
        background: "var(--surface-overlay)",
        border: "1px solid var(--accent-hot)",
        "border-radius": "8px",
        display: "flex",
        "flex-direction": "column",
        gap: "8px",
      }}
    >
      <div
        style={{
          display: "flex",
          "align-items": "baseline",
          "justify-content": "space-between",
          gap: "8px",
        }}
      >
        <strong style={{ color: "var(--accent-hot)" }}>preview</strong>
        <span
          style={{
            "font-family": "var(--font-mono)",
            "font-size": "0.75em",
            color: "var(--text-secondary)",
          }}
        >
          ~${p.estimated_total_usd.toFixed(4)} · paid ${p.lead_cost_usd.toFixed(4)}
        </span>
      </div>
      <div style={{ "font-size": "0.9em" }}>{p.goal}</div>
      <ol
        style={{
          margin: 0,
          padding: "0 0 0 18px",
          display: "flex",
          "flex-direction": "column",
          gap: "3px",
          "font-size": "0.85em",
        }}
      >
        <For each={p.subtasks}>
          {(t) => (
            <li>
              <strong
                style={{
                  "font-family": "var(--font-mono)",
                  color: "var(--accent-hot)",
                }}
              >
                {t.agent}
              </strong>
              <span style={{ color: "var(--text-secondary)", "margin-left": "6px" }}>
                {t.id}
              </span>
              <div style={{ color: "var(--text-secondary)" }}>{t.description}</div>
            </li>
          )}
        </For>
      </ol>
      <div style={{ display: "flex", gap: "6px" }}>
        <button onClick={onRun} title="Run the orchestrator with this plan">
          Run
        </button>
        <button onClick={onEdit} title="Restore the prompt to the input bar">
          Edit
        </button>
        <button
          onClick={onCancel}
          title="Discard the preview"
          style={{ "border-color": "var(--role-error)", color: "var(--role-error)" }}
        >
          Cancel
        </button>
      </div>
    </div>
  );
};

const WorkerRow: Component<{ worker: Worker }> = (props) => {
  const w = props.worker;
  return (
    <div
      style={{
        display: "grid",
        "grid-template-columns": "12px 1fr auto",
        gap: "8px",
        "align-items": "center",
        "font-size": "0.8em",
      }}
    >
      <span
        style={{
          width: "10px",
          height: "10px",
          "border-radius": "50%",
          background:
            w.status === "completed"
              ? "var(--role-tool-result)"
              : w.status === "failed"
              ? "var(--role-error)"
              : "var(--budget-mid)",
          animation: w.status === "running" ? "pulse 1.2s ease-in-out infinite" : undefined,
        }}
      />
      <div>
        <strong style={{ color: "var(--accent-hot)", "font-family": "var(--font-mono)" }}>
          {w.agent}
        </strong>
        <span style={{ color: "var(--text-secondary)", "margin-left": "6px" }}>
          {w.task_id}
        </span>
        <Show when={w.description}>
          <div
            style={{
              color: "var(--text-secondary)",
              "font-size": "0.85em",
              "margin-top": "1px",
            }}
          >
            {w.description}
          </div>
        </Show>
      </div>
      <Show when={w.status !== "running"}>
        <span
          style={{
            "font-family": "var(--font-mono)",
            "font-size": "0.75em",
            color: "var(--text-secondary)",
          }}
        >
          {w.status}
          <Show when={w.cost_usd > 0}>
            {" "}
            · ${w.cost_usd.toFixed(4)}
          </Show>
        </span>
      </Show>
    </div>
  );
};
