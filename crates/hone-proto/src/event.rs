use serde::{Deserialize, Serialize};

use crate::model::TokenUsage;
use crate::tool::ToolCallRequest;

/// Events streamed from a model provider during completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    Token { text: String },
    ToolCall(ToolCallRequest),
    Usage(TokenUsage),
    Done,
    Error { message: String },
    /// A tool call that the provider/CLI is executing on its own behalf
    /// (i.e. not dispatched through hone's ToolRegistry). The agent
    /// forwards these as `AgentEvent::ToolCallStart/End` for UI visibility
    /// but does NOT execute them — the provider already handles execution.
    ObservedToolStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Completion of a provider-handled tool call started by `ObservedToolStart`.
    ObservedToolEnd {
        id: String,
        result: String,
        is_error: bool,
    },
}

/// Events streamed from the agent loop to the UI layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A token from the model's response.
    Token { text: String },
    /// The model requested a tool call.
    ToolCallStart {
        id: String,
        name: String,
        /// Tool arguments as a JSON object.  Used by the TUI to extract file
        /// paths, line ranges, and other observability data without re-parsing
        /// the conversation log.
        #[serde(default)]
        args: serde_json::Value,
    },
    /// A tool call completed.
    ToolCallEnd {
        id: String,
        result: serde_json::Value,
        is_error: bool,
        /// Duration of this tool call in milliseconds.
        #[serde(default)]
        duration_ms: u64,
        /// Whether the result was served from the tool-result cache.
        ///
        /// `false` means the tool was actually executed (cache miss or
        /// non-cacheable tool).  Old serialized events that lack this field
        /// deserialize to `false` via `#[serde(default)]`.
        #[serde(default)]
        cache_hit: bool,
    },
    /// The turn completed successfully.
    Done {
        usage: TokenUsage,
        #[serde(default)]
        cost_usd: f64,
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
    /// Emitted once per session on the first successful completion to inform
    /// the TUI of the active model's real context-window size.
    ///
    /// The TUI uses this to populate `StatusInfo::context_limit`.  A default
    /// of 128 000 is used until this event arrives, so no context bar is shown
    /// as empty — the event refines the display once the provider responds.
    ModelInfo {
        context_window: u32,
    },
    /// Emitted in step mode just before a sequential tool call executes.
    /// The agent is paused and will not proceed until it receives a StepAction.
    ToolCallPaused {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Emitted once per turn when proactive context injection fires.
    ContextInjected {
        strategy: String,
        tokens_used: u32,
        /// (file_path, bm25_score) pairs from tier-2 BM25 retrieval.
        bm25_hits: Vec<(String, f32)>,
        /// "High", "Medium", or "Low"
        confidence: String,
    },
    /// Emitted once per turn when the memory store returns relevant entries.
    MemoryRecalled {
        /// (memory_id, short_snippet ≤ 80 chars) pairs.
        entries: Vec<(String, String)>,
    },
}

/// Action sent from the TUI to the agent when step mode is active and the
/// agent is paused before a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepAction {
    /// Execute the tool call normally.
    Proceed,
    /// Skip the tool call; the agent receives an empty/stub result.
    Skip,
}
