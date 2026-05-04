use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::agent::{Agent, AgentConfig};
use crate::agent_registry::AgentRegistry;
use crate::checkpoint::CheckpointStore;
use crate::error::AgentError;
use crate::plan_store::PlanStore;
use crate::task_plan::{OrchestratorResult, SubTask, TaskPlan, WorkerResult, WorkerStatus};
use hone_proto::{AgentEvent, ToolApprovalPolicy, TokenUsage};

/// Default per-worker timeout: 5 minutes.
const DEFAULT_WORKER_TIMEOUT_SECS: u64 = 300;

/// Orchestrates a multi-agent workflow: decompose → execute → merge.
pub struct Orchestrator {
    lead_config: AgentConfig,
    registry: AgentRegistry,
    max_workers: usize,
    /// Maximum number of worker agents allowed to run simultaneously.
    /// Enforced via a `tokio::sync::Semaphore`.  Defaults to `max_workers`.
    max_concurrent: usize,
    worker_budget: Option<f64>,
    total_budget: Option<f64>,
    plan_store: Option<PlanStore>,
    /// Per-worker timeout in seconds. Workers that exceed this are cancelled
    /// and recorded as `WorkerStatus::TimedOut`. Defaults to 300 s.
    worker_timeout_secs: u64,
    /// Optional crash-recovery checkpoint store.
    checkpoint_store: Option<CheckpointStore>,
}

impl Orchestrator {
    pub fn new(lead_config: AgentConfig, registry: AgentRegistry, max_workers: usize) -> Self {
        Self {
            lead_config,
            registry,
            max_concurrent: max_workers,
            max_workers,
            worker_budget: None,
            total_budget: None,
            plan_store: None,
            worker_timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
            checkpoint_store: None,
        }
    }

    /// Override the per-worker budget (USD). Workers without a budget run unbounded.
    pub fn with_worker_budget(mut self, budget: f64) -> Self {
        self.worker_budget = Some(budget);
        self
    }

    /// Set the total budget across all workers (USD). When exceeded, remaining
    /// workers are skipped with a `BudgetExceeded` status.
    pub fn with_total_budget(mut self, budget: f64) -> Self {
        self.total_budget = Some(budget);
        self
    }

    /// Attach a persistent plan store. When set, the plan is written to disk
    /// after decomposition and again after all workers complete.
    pub fn with_plan_store(mut self, store: PlanStore) -> Self {
        self.plan_store = Some(store);
        self
    }

    /// Cap the number of sub-agents that may run concurrently.
    ///
    /// The effective limit is `n.min(max_workers)` — it cannot exceed the
    /// batch size.  Setting `n = 1` serialises all workers, which is useful
    /// when calling a rate-limited provider.
    pub fn with_max_concurrent(mut self, n: usize) -> Self {
        self.max_concurrent = n;
        self
    }

    /// Set the per-worker wall-clock timeout in seconds.
    ///
    /// Workers that exceed the limit are cancelled and recorded with
    /// `WorkerStatus::TimedOut`.  The orchestrator then continues with the
    /// remaining tasks rather than aborting the whole run.
    pub fn with_worker_timeout(mut self, secs: u64) -> Self {
        self.worker_timeout_secs = secs;
        self
    }

    /// Attach a crash-recovery checkpoint store.
    ///
    /// When set, the orchestration state is persisted to SQLite after each
    /// worker completes.  Use `Orchestrator::resume` to continue an
    /// interrupted run.
    pub fn with_checkpoint_store(mut self, store: CheckpointStore) -> Self {
        self.checkpoint_store = Some(store);
        self
    }

    // -----------------------------------------------------------------------
    // Public entry point
    // -----------------------------------------------------------------------

    #[tracing::instrument(skip(self, events))]
    pub async fn run(
        &mut self,
        user_request: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<OrchestratorResult, AgentError> {
        // 1. Decompose the request into a TaskPlan.
        let plan = self.decompose(user_request, events.clone()).await?;

        let _ = events
            .send(AgentEvent::PlanCreated {
                goal: plan.goal.clone(),
                subtask_count: plan.subtasks.len(),
            })
            .await;

        // Persist the initial plan (all subtasks pending) if a store is configured.
        if let Some(ref store) = self.plan_store {
            if let Err(e) = store.write_plan(&plan, &[]).await {
                tracing::warn!("plan_store: failed to write initial plan: {e}");
            }
        }

        // 2. Execute subtasks in dependency order.
        let workers = self.execute_plan(plan.clone(), None, events.clone()).await;

        // Persist final plan state with worker results.
        if let Some(ref store) = self.plan_store {
            if let Err(e) = store.write_plan(&plan, &workers).await {
                tracing::warn!("plan_store: failed to write final plan: {e}");
            }
        }

        // 3. Merge all worker results into a single response.
        let merged = self
            .merge_results(&plan.goal, &workers, events.clone())
            .await;

        let total_cost = workers.iter().map(|w| w.cost_usd).sum::<f64>();

        Ok(OrchestratorResult {
            goal: plan.goal,
            merged_response: merged,
            workers,
            total_cost_usd: total_cost,
        })
    }

    /// Resume an interrupted orchestration run from a checkpoint.
    ///
    /// Loads the saved `OrchestrationCheckpoint` identified by `run_id`,
    /// skips all already-completed tasks, and continues execution.  The
    /// final `OrchestratorResult` combines the previously-completed workers
    /// with the newly-completed ones.
    ///
    /// Returns `AgentError::Cancelled` when no checkpoint store is attached
    /// or the `run_id` is not found.
    #[tracing::instrument(skip(self, events))]
    pub async fn resume(
        &mut self,
        run_id: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<OrchestratorResult, AgentError> {
        let store = self
            .checkpoint_store
            .as_ref()
            .ok_or(AgentError::Cancelled)?;

        let checkpoint = store
            .load(run_id)
            .await
            .map_err(|_| AgentError::Cancelled)?
            .ok_or(AgentError::Cancelled)?;

        tracing::info!(
            run_id,
            completed = checkpoint.completed_workers.len(),
            pending = checkpoint.pending_task_ids.len(),
            "resuming orchestration from checkpoint"
        );

        let already_done: HashSet<String> = checkpoint
            .completed_workers
            .iter()
            .map(|w| w.task_id.clone())
            .collect();

        // Reconstruct the plan with only pending tasks.
        let remaining_plan = TaskPlan {
            goal: checkpoint.goal.clone(),
            subtasks: checkpoint
                .plan
                .subtasks
                .into_iter()
                .filter(|t| !already_done.contains(&t.id))
                .collect(),
        };

        let _ = events
            .send(AgentEvent::PlanCreated {
                goal: remaining_plan.goal.clone(),
                subtask_count: remaining_plan.subtasks.len(),
            })
            .await;

        // Pass the pre-accumulated cost so budget tracking is consistent.
        let mut new_workers = self
            .execute_plan(
                remaining_plan.clone(),
                Some(checkpoint.total_cost_usd),
                events.clone(),
            )
            .await;

        // Combine previous results with new ones.
        let mut all_workers = checkpoint.completed_workers;
        all_workers.append(&mut new_workers);

        let merged = self
            .merge_results(&checkpoint.goal, &all_workers, events.clone())
            .await;

        let total_cost = all_workers.iter().map(|w| w.cost_usd).sum::<f64>();

        Ok(OrchestratorResult {
            goal: checkpoint.goal,
            merged_response: merged,
            workers: all_workers,
            total_cost_usd: total_cost,
        })
    }

    // -----------------------------------------------------------------------
    // Decompose
    // -----------------------------------------------------------------------

    pub async fn decompose(
        &mut self,
        user_request: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TaskPlan, AgentError> {
        let agent_list: String = self
            .registry
            .list()
            .into_iter()
            .map(|d| format!("- {}: {}", d.name, d.description))
            .collect::<Vec<_>>()
            .join("\n");

        let decompose_prompt = format!(
            "You are a lead agent. Decompose the user's request into subtasks for specialist agents.\n\n\
             Available agents:\n{agent_list}\n\n\
             Output ONLY a JSON object (no markdown, no explanation):\n\
             {{\"goal\": \"...\", \"subtasks\": [{{\"id\": \"1\", \"description\": \"...\", \"agent\": \"agent-name\", \"context\": \"\", \"depends_on\": [], \"acceptance_tests\": [], \"budget_usd\": null, \"scope\": []}}]}}\n\n\
             Rules:\n\
             - Maximum 5 subtasks\n\
             - Use agent names from the list above\n\
             - If the task is simple, use just 1 subtask\n\
             - Identify dependencies between subtasks (use id strings)\n\
             - Populate acceptance_tests with 1-3 concrete, verifiable success criteria when the task has clear deliverables\n\
             - Populate scope with file paths the worker is allowed to modify when scope can be bounded; leave empty for broad tasks\n\
             - Set budget_usd if the subtask has a known cost ceiling; omit (null) otherwise\n\n\
             User request: {user_request}"
        );

        let mut lead = Agent::new(self.build_lead_config());
        let turn = lead.run_turn(&decompose_prompt, events).await?;

        let raw = turn.response.trim().to_owned();

        // Strip markdown code fences if present.
        let json_str = strip_code_fences(&raw);

        match serde_json::from_str::<TaskPlan>(json_str) {
            Ok(plan) => Ok(plan),
            Err(_) => {
                // Fallback: treat the entire request as a single subtask.
                Ok(single_subtask_plan(user_request, &self.registry))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Execute
    // -----------------------------------------------------------------------

    /// Execute a task plan, respecting dependency order, concurrency limits,
    /// per-worker timeouts, and the total budget cap.
    ///
    /// `prior_spend_usd` is the cost already accumulated from a resumed
    /// checkpoint; pass `None` (or `Some(0.0)`) for a fresh run.
    pub async fn execute_plan(
        &self,
        plan: TaskPlan,
        prior_spend_usd: Option<f64>,
        events: mpsc::Sender<AgentEvent>,
    ) -> Vec<WorkerResult> {
        let total_tasks = plan.subtasks.len();
        let mut pending: VecDeque<SubTask> = plan.subtasks.into_iter().collect();
        let mut completed: HashSet<String> = HashSet::new();
        let mut results: Vec<WorkerResult> = Vec::new();
        let mut spent_usd: f64 = prior_spend_usd.unwrap_or(0.0);

        // The effective concurrency cap cannot exceed the batch size.
        let concurrency = self.max_concurrent.min(self.max_workers).max(1);
        let semaphore = Arc::new(Semaphore::new(concurrency));

        while !pending.is_empty() {
            // Check total budget before starting next batch.
            if let Some(total) = self.total_budget {
                if spent_usd >= total {
                    tracing::warn!(
                        spent = spent_usd,
                        budget = total,
                        remaining = pending.len(),
                        "orchestrator total budget exceeded — skipping remaining tasks"
                    );
                    // Mark remaining tasks as budget-exceeded.
                    for task in pending {
                        results.push(WorkerResult {
                            task_id: task.id,
                            agent: task.agent,
                            status: WorkerStatus::Failed(format!(
                                "skipped: total budget ${:.4} exceeded (spent ${:.4})",
                                total, spent_usd
                            )),
                            response: String::new(),
                            usage: TokenUsage::default(),
                            cost_usd: 0.0,
                        });
                    }
                    break;
                }
            }

            // Gather tasks whose dependencies are all satisfied.
            let mut ready: Vec<SubTask> = Vec::new();
            let mut still_waiting: VecDeque<SubTask> = VecDeque::new();

            for task in pending {
                let deps_met = task.depends_on.iter().all(|dep| completed.contains(dep));
                if deps_met {
                    ready.push(task);
                } else {
                    still_waiting.push_back(task);
                }
            }
            pending = still_waiting;

            if ready.is_empty() {
                break;
            }

            // Chunk ready tasks by max_workers and spawn batches.
            // The semaphore provides the real-time cap within (and across) batches.
            for batch in ready.chunks(self.max_workers) {
                // Side-table: spawn-order index → (task_id, agent).
                // Each future returns `(idx, WorkerResult)`.  If a future panics,
                // `join_next` yields `Err(JoinError)` which carries no user data,
                // so we find the missing index by exclusion and look up the real
                // identifiers from `spawn_meta`.
                let mut spawn_meta: Vec<(String, String)> = Vec::new();
                let mut set: JoinSet<(usize, WorkerResult)> = JoinSet::new();

                for task in batch {
                    let task = task.clone();
                    let events = events.clone();
                    let worker_config = self.build_worker_config(&task);
                    // Per-task budget from the plan overrides the default worker budget.
                    let effective_budget = task.budget_usd.or(self.worker_budget);
                    let timeout_secs = self.worker_timeout_secs;
                    let sem = semaphore.clone();

                    let _ = events
                        .send(AgentEvent::WorkerStarted {
                            task_id: task.id.clone(),
                            agent: task.agent.clone(),
                            description: task.description.clone(),
                        })
                        .await;

                    // Record identity at spawn time so we can recover it even if
                    // the future panics (JoinError carries no payload).
                    let idx = spawn_meta.len();
                    spawn_meta.push((task.id.clone(), task.agent.clone()));

                    // Acquire a permit *before* spawning so the spawn happens
                    // only when there is capacity.  The permit is moved into
                    // the async block and dropped when the task finishes.
                    let permit = sem.acquire_owned().await.expect("semaphore never closed");

                    set.spawn(async move {
                        let result =
                            run_worker(task, worker_config, effective_budget, timeout_secs, events)
                                .await;
                        // Explicit drop documents the intent; the borrow-checker
                        // guarantees the permit is released here regardless.
                        drop(permit);
                        (idx, result)
                    });
                }

                // Collect all outcomes; panicked tasks are identified by the
                // indices absent from `completed_indices`.
                let total_spawned = spawn_meta.len();
                let mut completed_indices: Vec<usize> = Vec::with_capacity(total_spawned);
                let mut batch_results: Vec<(usize, WorkerResult)> =
                    Vec::with_capacity(total_spawned);

                while let Some(res) = set.join_next().await {
                    match res {
                        Ok((idx, worker_result)) => {
                            completed_indices.push(idx);
                            batch_results.push((idx, worker_result));
                        }
                        Err(join_err) => {
                            // A future panicked.  We cannot tell *which* index
                            // panicked from the JoinError alone, so we log it and
                            // resolve missing indices after the loop.
                            tracing::error!("worker task panicked (join error): {join_err}");
                        }
                    }
                }

                // Synthesize failure results for any panicked tasks.
                let completed_set: std::collections::HashSet<usize> =
                    completed_indices.into_iter().collect();
                for idx in 0..total_spawned {
                    if !completed_set.contains(&idx) {
                        let (task_id, agent) = spawn_meta[idx].clone();
                        batch_results.push((
                            idx,
                            WorkerResult {
                                task_id,
                                agent,
                                status: WorkerStatus::Failed("worker task panicked".to_owned()),
                                response: String::new(),
                                usage: TokenUsage::default(),
                                cost_usd: 0.0,
                            },
                        ));
                    }
                }

                // Sort by spawn order so checkpointing and budget progress are
                // deterministic, then process each result.
                batch_results.sort_by_key(|(idx, _)| *idx);

                for (_, worker_result) in batch_results {
                    spent_usd += worker_result.cost_usd;
                    completed.insert(worker_result.task_id.clone());

                    // Persist state after each completion when a checkpoint store
                    // is attached.
                    if let Some(ref cp_store) = self.checkpoint_store {
                        let pending_ids: Vec<String> =
                            pending.iter().map(|t| t.id.clone()).collect();
                        if let Err(e) = cp_store
                            .save_progress(&results, &pending_ids, spent_usd)
                            .await
                        {
                            tracing::warn!("checkpoint: failed to save progress: {e}");
                        }
                    }

                    results.push(worker_result);

                    // Emit budget progress.
                    let _ = events
                        .send(AgentEvent::BudgetProgress {
                            spent_usd,
                            budget_usd: self.total_budget,
                            workers_completed: results.len(),
                            workers_total: total_tasks,
                        })
                        .await;
                }
            }
        }

        results
    }

    // -----------------------------------------------------------------------
    // Merge
    // -----------------------------------------------------------------------

    pub async fn merge_results(
        &mut self,
        goal: &str,
        results: &[WorkerResult],
        events: mpsc::Sender<AgentEvent>,
    ) -> String {
        let summaries: String = results
            .iter()
            .map(|r| {
                let status = match &r.status {
                    WorkerStatus::Completed => "completed".to_owned(),
                    WorkerStatus::Failed(msg) => format!("failed: {msg}"),
                    WorkerStatus::TimedOut(msg) => format!("timed out: {msg}"),
                };
                format!(
                    "### Task {} (agent: {}, status: {})\n{}",
                    r.task_id, r.agent, status, r.response
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let merge_prompt = format!(
            "You are the lead agent. Based on the results from specialist workers, \
             synthesize a final, user-facing response.\n\n\
             Original goal: {goal}\n\n\
             Worker results:\n{summaries}\n\n\
             Provide a concise, integrated response."
        );

        let mut lead = Agent::new(self.build_lead_config());
        match lead.run_turn(&merge_prompt, events).await {
            Ok(turn) => turn.response,
            Err(e) => format!("Merge failed: {e}"),
        }
    }

    // -----------------------------------------------------------------------
    // Config builders
    // -----------------------------------------------------------------------

    fn build_lead_config(&self) -> AgentConfig {
        // Lead agent also gets a fresh empty tool registry — orchestration
        // happens via the agent loop, not tool calls.
        AgentConfig {
            provider: Arc::clone(&self.lead_config.provider),
            tools: hone_tools::ToolRegistry::new(),
            sandbox: Arc::clone(&self.lead_config.sandbox),
            system_prompt: self.lead_config.system_prompt.clone(),
            model: self.lead_config.model.clone(),
            max_tokens: self.lead_config.max_tokens,
            temperature: self.lead_config.temperature,
            budget_limit: self.lead_config.budget_limit,
            pricing: self.lead_config.pricing.clone(),
            working_dir: self.lead_config.working_dir.clone(),
            memory_store: None,
            retry_config: self.lead_config.retry_config.clone(),
            index: None,
            inject_context: false,
            skeleton_strategy: Default::default(),
            egress_log: None,
            approval_policy: ToolApprovalPolicy::Auto,
            step_rx: None,
            hooks: None,
            skills: None,
            mode: crate::plan_mode::AgentMode::Build,
        }
    }

    fn build_worker_config(&self, task: &SubTask) -> AgentConfig {
        // Look up the agent definition; fall back to the lead system prompt.
        let base = match self.registry.get(&task.agent) {
            Some(def) => def.system_prompt.as_str(),
            None => self.lead_config.system_prompt.as_str(),
        };

        let mut system_prompt = format!(
            "{base}\n\n## Task\n{}\n\n## Context\n{}",
            task.description, task.context
        );

        if !task.acceptance_tests.is_empty() {
            system_prompt.push_str("\n\n## Acceptance Criteria\n");
            for (i, test) in task.acceptance_tests.iter().enumerate() {
                system_prompt.push_str(&format!("{}. {}\n", i + 1, test));
            }
            system_prompt.push_str("\nVerify all criteria are met before reporting completion.\n");
        }

        if !task.scope.is_empty() {
            system_prompt.push_str("\n\n## Scope\n");
            system_prompt.push_str("You may only modify these files:\n");
            for path in &task.scope {
                system_prompt.push_str(&format!("- {path}\n"));
            }
        }

        // Workers get a fresh empty tool registry — they are specialist agents
        // focused on reasoning tasks. ToolRegistry does not implement Clone.
        AgentConfig {
            provider: Arc::clone(&self.lead_config.provider),
            tools: hone_tools::ToolRegistry::new(),
            sandbox: Arc::clone(&self.lead_config.sandbox),
            system_prompt,
            model: self.lead_config.model.clone(),
            max_tokens: self.lead_config.max_tokens,
            temperature: self.lead_config.temperature,
            budget_limit: self.worker_budget,
            pricing: self.lead_config.pricing.clone(),
            working_dir: self.lead_config.working_dir.clone(),
            memory_store: None,
            retry_config: self.lead_config.retry_config.clone(),
            index: None,
            inject_context: false,
            skeleton_strategy: Default::default(),
            egress_log: None,
            approval_policy: ToolApprovalPolicy::Auto,
            step_rx: None,
            hooks: None,
            skills: None,
            mode: crate::plan_mode::AgentMode::Build,
        }
    }
}

// ---------------------------------------------------------------------------
// Worker execution (free function — runs in spawned task)
// ---------------------------------------------------------------------------

async fn run_worker(
    task: SubTask,
    mut config: AgentConfig,
    worker_budget: Option<f64>,
    timeout_secs: u64,
    events: mpsc::Sender<AgentEvent>,
) -> WorkerResult {
    let task_id = task.id.clone();
    let agent_name = task.agent.clone();

    // Apply the effective budget to the agent config so the agent's
    // CostAccumulator will enforce it during the turn.
    if worker_budget.is_some() {
        config.budget_limit = worker_budget;
    }

    let mut agent = Agent::new(config);
    let prompt = format!("{}\n\n{}", task.description, task.context);
    let timeout = Duration::from_secs(timeout_secs);

    match tokio::time::timeout(timeout, agent.run_turn(&prompt, events.clone())).await {
        Ok(Ok(turn)) => {
            let cost = turn.cost_usd;

            let _ = events
                .send(AgentEvent::WorkerCompleted {
                    task_id: task_id.clone(),
                    agent: agent_name.clone(),
                    status: "completed".to_owned(),
                    cost_usd: cost,
                })
                .await;

            WorkerResult {
                task_id,
                agent: agent_name,
                status: WorkerStatus::Completed,
                response: turn.response,
                usage: turn.usage,
                cost_usd: cost,
            }
        }
        Ok(Err(e)) => {
            let err_msg = e.to_string();
            let _ = events
                .send(AgentEvent::WorkerCompleted {
                    task_id: task_id.clone(),
                    agent: agent_name.clone(),
                    status: format!("failed: {err_msg}"),
                    cost_usd: 0.0,
                })
                .await;

            WorkerResult {
                task_id,
                agent: agent_name,
                status: WorkerStatus::Failed(err_msg),
                response: String::new(),
                usage: TokenUsage::default(),
                cost_usd: 0.0,
            }
        }
        Err(_elapsed) => {
            let msg = format!("exceeded {timeout_secs}s");
            tracing::warn!(
                task_id = %task_id,
                agent = %agent_name,
                timeout_secs,
                "worker timed out"
            );

            let _ = events
                .send(AgentEvent::WorkerCompleted {
                    task_id: task_id.clone(),
                    agent: agent_name.clone(),
                    status: "timed_out".to_owned(),
                    cost_usd: 0.0,
                })
                .await;

            WorkerResult {
                task_id,
                agent: agent_name,
                status: WorkerStatus::TimedOut(msg),
                response: String::new(),
                usage: TokenUsage::default(),
                cost_usd: 0.0,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip markdown code fences (```json ... ``` or ``` ... ```) from a string.
fn strip_code_fences(s: &str) -> &str {
    let trimmed = s.trim();

    // Try ```json\n...\n``` or ```\n...\n```
    for prefix in &["```json\n", "```\n", "```json", "```"] {
        if let Some(after_open) = trimmed.strip_prefix(prefix) {
            if let Some(before_close) = after_open.strip_suffix("```") {
                return before_close.trim();
            }
            // Close fence may be on its own line.
            if let Some(pos) = after_open.rfind("\n```") {
                return after_open[..pos].trim();
            }
        }
    }

    trimmed
}

/// Build a trivial one-subtask plan using the first available agent (or a
/// generic fallback name when the registry is empty).
fn single_subtask_plan(request: &str, registry: &AgentRegistry) -> TaskPlan {
    let agent_name = registry
        .list()
        .into_iter()
        .next()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "assistant".to_owned());

    TaskPlan {
        goal: request.to_owned(),
        subtasks: vec![SubTask {
            id: "1".to_owned(),
            description: request.to_owned(),
            agent: agent_name,
            context: String::new(),
            depends_on: Vec::new(),
            acceptance_tests: Vec::new(),
            budget_usd: None,
            scope: Vec::new(),
        }],
    }
}
