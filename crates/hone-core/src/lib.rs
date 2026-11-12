mod agent;
pub mod agent_def;
pub mod agent_name;
pub mod agent_registry;
pub mod audit;
pub mod checkpoint;
pub mod clarify;
mod code_mode;
pub mod complexity;
mod context;
mod receipts;
pub mod strategy;
pub mod context_injector;
mod cost;
pub mod doom_loop;
pub mod egress_log;
pub mod ensemble;
mod error;
pub mod handoff;
pub mod hierarchical_skeleton;
pub mod hooks;
pub mod injection_detector;
pub mod learning;
pub mod memory_format;
pub mod memory_index;
pub mod memory_key;
pub mod memory_worthy;
pub mod metrics;
pub mod orchestrator;
pub mod plan_mode;
pub mod plan_store;
pub mod queue_client;
pub mod ralph_loop;
pub mod rbac;
pub mod recovery;
pub mod relevance;
pub mod retry;
pub mod reviewer;
pub mod self_refine;
pub mod session_approvals;
pub mod task_plan;
pub mod test_loop;
mod turn;

/// Context receipts with their per-agent ordinals (see
/// `AgentHandle::context_receipts_after`).
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiptBatch {
    /// Identifies the agent the ordinals belong to.
    pub agent_id: String,
    /// `(ordinal, receipt)`, ordinals 1-based and ascending.
    pub items: Vec<(u64, hone_proto::ContextReceipt)>,
}

#[allow(deprecated)]
pub use agent::{Agent, AgentConfig, AgentHandle, RunSettings, is_read_only_tool};
pub use agent_def::{AgentDef, AgentDefError};
pub use agent_registry::{AgentRegistry, AllowlistRejection, filtered_registry};
pub use audit::{AuditEntry, AuditError, AuditFilter, AuditKind, AuditLog, InMemoryAuditLog};
pub use checkpoint::{CheckpointError, CheckpointStore, OrchestrationCheckpoint};
pub use clarify::ClarificationDetector;
pub use code_mode::{CodeModeOutput, CodeModeRunner};
pub use complexity::{ComplexityClassifier, ComplexityTier};
pub use context::{
    CONTEXT_WINDOW_ENV, ContextConfig, ContextManager, MAX_CONTEXT_WINDOW, MIN_CONTEXT_WINDOW,
    TaskPhase, ToolPhaseKind, classify_tool_call, estimate_text_tokens, parse_context_window,
};
pub use context_injector::{
    ContextInjector, InjectionResult, RetrievalConfidence, SkeletonStrategy,
};
pub use cost::CostAccumulator;
pub use egress_log::{EgressEntry, EgressLog, SpanKind, TraceContext};
pub use ensemble::{AgentEnsemble, EnsembleResult};
pub use error::AgentError;
pub use hooks::{HookError, HookEvent, HookOutcome, HookRegistry, HookSpec};
pub use injection_detector::{InjectionDetector, InjectionRisk};
pub use learning::{AdaptiveClassifier, OutcomeRecord};
pub use memory_key::friendly_memory_key;
pub use metrics::{ContextMetrics, MetricsSnapshot};
pub use orchestrator::Orchestrator;
pub use plan_mode::{AgentMode, PlanModeStore, SavedPlan, read_only_registry, slugify};
pub use plan_store::PlanStore;
pub use session_approvals::SessionApprovals;
pub use hone_tools::policy_gate::{GateDecision, PolicyConfig, decide};
pub use queue_client::{
    ClaimedTaskWire, CompletionRequestBody, PlanEvent, PlanStatusSnapshot, QueueClient,
    QueueClientError, WorkerResultWire,
};
pub use rbac::{RbacError, RbacPolicy, Role, ToolAccess};
pub use recovery::{FailureScenario, RecoveryAction};
pub use retry::{RetryConfig, is_transient_error, retry};
pub use reviewer::{ReviewIssue, ReviewResult, ReviewSeverity, ReviewerAgent};
pub use self_refine::{RefineResult, SelfRefineLoop};
pub use task_plan::{OrchestratorResult, SubTask, TaskPlan, WorkerResult, WorkerStatus};
pub use test_loop::{
    GreenCommands, GreenContract, GreenLevel, TestLoopResult, TestStatus, TestVerificationLoop,
};
pub use turn::TurnResult;
pub mod skills;
pub use skills::{Skill, SkillError, SkillRegistry};

/// Heuristic per-worker cost used by the orchestrate dry-run preview to
/// project total spend. Operators can tune this by passing an explicit
/// `total_budget_usd` on the request; this constant is the fallback.
pub const DEFAULT_WORKER_COST_USD: f64 = 0.05;
