// Shared orchestrator-progress state.
//
// Phase 2B: ChatPanel writes plan / worker / budget events into here as
// they arrive from the SSE stream; <WorkerProgressPanel> reads. Stays
// per-turn — the next /run resets the plan.

import { createSignal } from "solid-js";

export type WorkerStatus = "running" | "completed" | "failed";

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

/** Wipe all orchestrator state. Called at the start of each new turn. */
export function resetOrchestratorState(): void {
  setPlanInternal(null);
  setWorkersInternal([]);
  setBudgetInternal(null);
}
