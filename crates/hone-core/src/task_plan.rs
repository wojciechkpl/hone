use hone_proto::TokenUsage;
use serde::{Deserialize, Serialize};

/// A single unit of work assigned to a specialist agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: String,
    pub description: String,
    pub agent: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Explicit acceptance criteria for this subtask.
    /// Workers are expected to verify these before reporting completion.
    #[serde(default)]
    pub acceptance_tests: Vec<String>,
    /// Maximum spend in USD. None means no per-task cap (session budget still applies).
    #[serde(default)]
    pub budget_usd: Option<f64>,
    /// Files the worker is allowed to modify. Empty = no restriction.
    #[serde(default)]
    pub scope: Vec<String>,
}

/// The full decomposition of a user request into subtasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub goal: String,
    pub subtasks: Vec<SubTask>,
}

/// The outcome of a single worker agent run.
#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub task_id: String,
    pub agent: String,
    pub status: WorkerStatus,
    pub response: String,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

/// Whether the worker completed successfully or encountered an error.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerStatus {
    Completed,
    Failed(String),
}

/// The final output of the orchestrator after all workers have finished.
#[derive(Debug, Clone)]
pub struct OrchestratorResult {
    pub goal: String,
    pub merged_response: String,
    pub workers: Vec<WorkerResult>,
    pub total_cost_usd: f64,
}
