use hone_proto::TokenUsage;
use serde::{Deserialize, Serialize};

/// A single unit of work assigned to a specialist agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub id: String,
    pub description: String,
    pub agent: String,
    pub context: String,
    pub depends_on: Vec<String>,
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
