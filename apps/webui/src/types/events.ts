// Wire-level shape of `hone_proto::AgentEvent`. Mirrors the serde
// representation (`tag = "type", rename_all = "snake_case"`) — keep in
// sync with `crates/hone-proto/src/event.rs`.

export type TokenUsage = {
  input?: number;
  output?: number;
  cache_read?: number;
  cache_creation?: number;
};

/**
 * Plan payload attached to `done` events when the caller requested a
 * dry-run (`POST /orchestrate?dry_run=true`). Mirrors `hone_core::TaskPlan`.
 */
export type DryRunPlan = {
  goal: string;
  subtasks: Array<DryRunSubtask>;
};

export type DryRunSubtask = {
  id: string;
  agent: string;
  description: string;
  /** Empty / missing on older daemons; treat absent as []. */
  depends_on?: string[];
};

export type AgentEvent =
  | { type: "token"; text: string }
  | { type: "tool_call_start"; id: string; name: string; args?: unknown }
  | {
      type: "tool_call_end";
      id: string;
      result: unknown;
      is_error: boolean;
      duration_ms?: number;
      cache_hit?: boolean;
    }
  | {
      type: "done";
      usage: TokenUsage;
      cost_usd?: number;
      plan?: DryRunPlan;
    }
  | { type: "error"; message: string }
  | { type: "approval_required"; action: string; description: string }
  | { type: "plan_created"; goal: string; subtask_count: number }
  | {
      type: "worker_started";
      task_id: string;
      agent: string;
      description: string;
    }
  | {
      type: "worker_completed";
      task_id: string;
      agent: string;
      status: string;
      cost_usd: number;
    }
  | {
      type: "budget_progress";
      spent_usd: number;
      budget_usd: number | null;
      workers_completed: number;
      workers_total: number;
    }
  | { type: "model_info"; context_window: number }
  | { type: "tool_call_paused"; id: string; name: string; args: unknown }
  | { type: "memory_recalled"; entries: Array<[string, string]> };
