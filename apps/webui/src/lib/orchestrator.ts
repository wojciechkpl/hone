// Shared orchestrator-progress state.
//
// Phase 2B: ChatPanel writes plan / worker / budget events into here as
// they arrive from the SSE stream; <WorkerProgressPanel> reads. Stays
// per-turn — the next /run resets the plan.

import { createSignal } from "solid-js";
import type { DryRunPlan } from "../types/events";

export type WorkerStatus = "running" | "completed" | "failed";

/**
 * Heuristic per-worker cost used by the orchestrate dry-run preview to
 * project total spend. Mirrors `hone_core::DEFAULT_WORKER_COST_USD`.
 */
export const DEFAULT_WORKER_COST_USD = 0.05;

/**
 * State for a pending plan-preview gate: the decomposed plan, the actual
 * cost paid for the lead-LLM decompose call, the projected total spend
 * if the user clicks Run, and the original prompt (so Run/Edit can
 * reuse it).
 */
export type DryRunPreview = {
  prompt: string;
  goal: string;
  subtasks: DryRunPlan["subtasks"];
  lead_cost_usd: number;
  estimated_total_usd: number;
};

export type Worker = {
  task_id: string;
  agent: string;
  description: string;
  status: WorkerStatus;
  cost_usd: number;
};

export type Plan = {
  goal: string;
  subtask_count: number;
};

export type Budget = {
  spent_usd: number;
  budget_usd: number | null;
  workers_completed: number;
  workers_total: number;
};

const [plan, setPlanInternal] = createSignal<Plan | null>(null);
export const usePlan = () => plan;

const [workers, setWorkersInternal] = createSignal<Worker[]>([]);
export const useWorkers = () => workers;

const [budget, setBudgetInternal] = createSignal<Budget | null>(null);
export const useBudget = () => budget;

export function setPlan(p: Plan | null): void {
  setPlanInternal(p);
}

export function startWorker(w: Omit<Worker, "status" | "cost_usd">): void {
  setWorkersInternal((arr) => [
    ...arr,
    { ...w, status: "running", cost_usd: 0 },
  ]);
}

export function completeWorker(
  task_id: string,
  status: "completed" | "failed",
  cost_usd: number,
): void {
  setWorkersInternal((arr) =>
    arr.map((w) =>
      w.task_id === task_id ? { ...w, status, cost_usd } : w,
    ),
  );
}

export function setBudget(b: Budget | null): void {
  setBudgetInternal(b);
}

const [dryRunPreview, setDryRunPreviewInternal] = createSignal<DryRunPreview | null>(null);
export const useDryRunPreview = () => dryRunPreview;

/**
 * Build a DryRunPreview from the daemon's dry-run `done` event payload.
 * `lead_cost_usd` comes from the same event's `cost_usd`. `prompt` is
 * the original user prompt (the daemon doesn't echo it back).
 */
export function buildDryRunPreview(
  prompt: string,
  plan: DryRunPlan,
  lead_cost_usd: number,
): DryRunPreview {
  return {
    prompt,
    goal: plan.goal,
    subtasks: plan.subtasks,
    lead_cost_usd,
    estimated_total_usd:
      lead_cost_usd + plan.subtasks.length * DEFAULT_WORKER_COST_USD,
  };
}

export function setDryRunPreview(preview: DryRunPreview | null): void {
  setDryRunPreviewInternal(preview);
}

export function clearDryRunPreview(): void {
  setDryRunPreviewInternal(null);
}

/**
 * Action signal driven by `<WorkerProgressPanel>` so `<ChatPanel>` (the
 * top-level dispatcher) can react to Run / Edit / Cancel without prop
 * drilling. Drained via `takePendingPlanAction()` so a single action
 * fires once.
 */
export type PlanAction =
  | { kind: "run"; preview: DryRunPreview }
  | { kind: "edit"; preview: DryRunPreview }
  | { kind: "cancel" };

const [pendingPlanAction, setPendingPlanActionInternal] =
  createSignal<PlanAction | null>(null);
export const usePendingPlanAction = () => pendingPlanAction;

export function setPendingPlanAction(action: PlanAction | null): void {
  setPendingPlanActionInternal(action);
}

/** Returns the action if any, then clears it. */
export function takePendingPlanAction(): PlanAction | null {
  const v = pendingPlanAction();
  if (v) setPendingPlanActionInternal(null);
  return v;
}

/** Wipe all orchestrator state. Called at the start of each new turn. */
export function resetOrchestratorState(): void {
  setPlanInternal(null);
  setWorkersInternal([]);
  setBudgetInternal(null);
  setDryRunPreviewInternal(null);
}
