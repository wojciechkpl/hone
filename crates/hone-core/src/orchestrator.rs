use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::agent::{Agent, AgentConfig};
use crate::agent_registry::AgentRegistry;
use crate::error::AgentError;
use crate::plan_store::PlanStore;
use crate::task_plan::{OrchestratorResult, SubTask, TaskPlan, WorkerResult, WorkerStatus};
use hone_proto::{AgentEvent, TokenUsage};

/// Orchestrates a multi-agent workflow: decompose → execute → merge.
pub struct Orchestrator {
    lead_config: AgentConfig,
    registry: AgentRegistry,
    max_workers: usize,
    worker_budget: Option<f64>,
    plan_store: Option<PlanStore>,
}

impl Orchestrator {
    pub fn new(lead_config: AgentConfig, registry: AgentRegistry, max_workers: usize) -> Self {
        Self {
            lead_config,
            registry,
            max_workers,
            worker_budget: None,
            plan_store: None,
        }
    }

    /// Override the per-worker budget (USD). Workers without a budget run unbounded.
    pub fn with_worker_budget(mut self, budget: f64) -> Self {
        self.worker_budget = Some(budget);
        self
    }

    /// Attach a persistent plan store. When set, the plan is written to disk
    /// after decomposition and again after all workers complete.
    pub fn with_plan_store(mut self, store: PlanStore) -> Self {
        self.plan_store = Some(store);
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
        let workers = self.execute_plan(plan.clone(), events.clone()).await;

        // Persist final plan state with worker results.
        if let Some(ref store) = self.plan_store {
            if let Err(e) = store.write_plan(&plan, &workers).await {
                tracing::warn!("plan_store: failed to write final plan: {e}");
            }
        }

        // 3. Merge all worker results into a single response.
        let merged = self.merge_results(&plan.goal, &workers, events.clone()).await;

        let total_cost = workers.iter().map(|w| w.cost_usd).sum::<f64>();

        Ok(OrchestratorResult {
            goal: plan.goal,
            merged_response: merged,
            workers,
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
             {{\"goal\": \"...\", \"subtasks\": [{{\"id\": \"1\", \"description\": \"...\", \"agent\": \"agent-name\", \"context\": \"\", \"depends_on\": []}}]}}\n\n\
             Rules:\n\
             - Maximum 5 subtasks\n\
             - Use agent names from the list above\n\
             - If the task is simple, use just 1 subtask\n\
             - Identify dependencies between subtasks (use id strings)\n\n\
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

    pub async fn execute_plan(
        &self,
        plan: TaskPlan,
        events: mpsc::Sender<AgentEvent>,
    ) -> Vec<WorkerResult> {
        let mut pending: VecDeque<SubTask> = plan.subtasks.into_iter().collect();
        let mut completed: HashSet<String> = HashSet::new();
        let mut results: Vec<WorkerResult> = Vec::new();

        while !pending.is_empty() {
            // Gather tasks whose dependencies are all satisfied.
            let mut ready: Vec<SubTask> = Vec::new();
            let mut still_waiting: VecDeque<SubTask> = VecDeque::new();

            for task in pending {
                let deps_met = task
                    .depends_on
                    .iter()
                    .all(|dep| completed.contains(dep));
                if deps_met {
                    ready.push(task);
                } else {
                    still_waiting.push_back(task);
                }
            }
            pending = still_waiting;

            if ready.is_empty() {
                // No progress possible — dependency cycle or missing dep. Stop.
                break;
            }

            // Chunk ready tasks by max_workers and spawn batches.
            for batch in ready.chunks(self.max_workers) {
                let mut set: JoinSet<WorkerResult> = JoinSet::new();

                for task in batch {
                    let task = task.clone();
                    let events = events.clone();
                    let worker_config = self.build_worker_config(&task);
                    let worker_budget = self.worker_budget;

                    let _ = events
                        .send(AgentEvent::WorkerStarted {
                            task_id: task.id.clone(),
                            agent: task.agent.clone(),
                            description: task.description.clone(),
                        })
                        .await;

                    set.spawn(async move {
                        run_worker(task, worker_config, worker_budget, events).await
                    });
                }

                while let Some(res) = set.join_next().await {
                    match res {
                        Ok(worker_result) => {
                            completed.insert(worker_result.task_id.clone());
                            results.push(worker_result);
                        }
                        Err(join_err) => {
                            // JoinError — treat as failed worker with unknown id.
                            results.push(WorkerResult {
                                task_id: "unknown".to_owned(),
                                agent: "unknown".to_owned(),
                                status: WorkerStatus::Failed(join_err.to_string()),
                                response: String::new(),
                                usage: TokenUsage::default(),
                                cost_usd: 0.0,
                            });
                        }
                    }
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
        }
    }

    fn build_worker_config(&self, task: &SubTask) -> AgentConfig {
        // Look up the agent definition; fall back to the lead system prompt.
        let system_prompt = match self.registry.get(&task.agent) {
            Some(def) => format!(
                "{}\n\n## Task\n{}\n\n## Context\n{}",
                def.system_prompt, task.description, task.context
            ),
            None => format!(
                "{}\n\n## Task\n{}\n\n## Context\n{}",
                self.lead_config.system_prompt, task.description, task.context
            ),
        };

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
        }
    }
}

// ---------------------------------------------------------------------------
// Worker execution (free function — runs in spawned task)
// ---------------------------------------------------------------------------

async fn run_worker(
    task: SubTask,
    config: AgentConfig,
    _worker_budget: Option<f64>,
    events: mpsc::Sender<AgentEvent>,
) -> WorkerResult {
    let task_id = task.id.clone();
    let agent_name = task.agent.clone();

    let mut agent = Agent::new(config);
    let prompt = format!("{}\n\n{}", task.description, task.context);

    match agent.run_turn(&prompt, events.clone()).await {
        Ok(turn) => {
            let cost = turn.cost_usd;
            let status_str = "completed".to_owned();

            let _ = events
                .send(AgentEvent::WorkerCompleted {
                    task_id: task_id.clone(),
                    agent: agent_name.clone(),
                    status: status_str,
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
        Err(e) => {
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
        }],
    }
}
