import { For, Show, type Component } from "solid-js";
import {
  useBudget,
  usePlan,
  useWorkers,
  type Worker,
} from "../lib/orchestrator";

export const WorkerProgressPanel: Component = () => {
  const plan = usePlan();
  const workers = useWorkers();
  const budget = useBudget();

  // Hide entirely when no orchestration is running.
  return (
    <Show when={plan() || workers().length > 0}>
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
