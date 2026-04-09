use serde::{Deserialize, Serialize};

use crate::model::TokenUsage;
use crate::tool::ToolCallRequest;

/// Events streamed from a model provider during completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    Token { text: String },
    ToolCall(ToolCallRequest),
    Usage(TokenUsage),
    Done,
    Error { message: String },
}

/// Events streamed from the agent loop to the UI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A token from the model's response.
    Token { text: String },
    /// The model requested a tool call.
    ToolCallStart {
        id: String,
        name: String,
    },
    /// A tool call completed.
    ToolCallEnd {
        id: String,
        result: serde_json::Value,
        is_error: bool,
    },
    /// The turn completed successfully.
    Done {
        usage: TokenUsage,
    },
    /// An error occurred during the turn.
    Error {
        message: String,
    },
    /// The agent is waiting for user approval.
    ApprovalRequired {
        action: String,
        description: String,
    },
    /// The lead agent produced a decomposition plan.
    PlanCreated {
        goal: String,
        subtask_count: usize,
    },
    /// A worker agent started executing a subtask.
    WorkerStarted {
        task_id: String,
        agent: String,
        description: String,
    },
    /// A worker agent finished a subtask.
    WorkerCompleted {
        task_id: String,
        agent: String,
        status: String,
        cost_usd: f64,
    },
    /// Budget progress update from the orchestrator.
    BudgetProgress {
        spent_usd: f64,
        budget_usd: Option<f64>,
        workers_completed: usize,
        workers_total: usize,
    },
}
