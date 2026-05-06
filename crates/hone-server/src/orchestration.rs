//! `POST /orchestrate` — multi-agent orchestrated execution endpoint with
//! SSE streaming.
//!
//! Sibling to `/run`. Where `/run` invokes a single Agent against the user's
//! prompt, `/orchestrate` invokes the [`Orchestrator`] which:
//!
//! 1. Decomposes the prompt into a `TaskPlan` (emits `PlanCreated`)
//! 2. Spawns worker agents per subtask (emits `WorkerStarted` /
//!    `WorkerCompleted`)
//! 3. Tracks budget across the run (emits `BudgetProgress`)
//! 4. Merges all worker results into a final response
//!
//! All four extra event variants flow through the same `mpsc::Sender<AgentEvent>`
//! the rest of the server uses, so the SSE wire format is identical to `/run`.
//!
//! The frontend uses `PlanCreated` to seed a task-tree sidebar and then
//! flips node statuses on `WorkerStarted`/`WorkerCompleted`.

use std::convert::Infallible;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream;
use hone_core::agent_registry::AgentRegistry;
use hone_core::orchestrator::Orchestrator;
use hone_core::{AgentConfig, AgentMode, RetryConfig, SkeletonStrategy};
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::{AnthropicProvider, ModelProvider, PricingTable};
use hone_tools::{
    EditFileTool, GlobTool, GrepTool, ReadFileTool, ShellTool, ToolRegistry, WriteFileTool,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::execution::{HistoryMessage, build_sandbox};
use crate::state::AppState;

/// Default lead-agent system prompt for orchestrated runs. Distinct from
/// `/run`'s prompt because the lead agent's job is *decomposition*, not
/// direct task execution.
const ORCHESTRATOR_SYSTEM_PROMPT: &str = "You are Hone's orchestrator. \
    Decompose the user's request into a small number of focused subtasks, \
    each one assigned to a specialist worker agent.";

/// Default cap on parallel worker agents. Operators can override per-request
/// via `max_workers`.
const DEFAULT_MAX_WORKERS: usize = 4;

/// Request body for `POST /orchestrate`.
#[derive(Debug, Deserialize)]
pub struct OrchestrateRequest {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub history: Vec<HistoryMessage>,
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Maximum number of worker subtasks the lead agent is allowed to create
    /// (and therefore the upper bound on parallelism). Defaults to
    /// [`DEFAULT_MAX_WORKERS`].
    #[serde(default)]
    pub max_workers: Option<usize>,
    /// Total cost ceiling across all workers. When exceeded, remaining
    /// workers are short-circuited with a `BudgetExceeded` status.
    #[serde(default)]
    pub total_budget_usd: Option<f64>,
}

/// Resolve an optional `working_dir` against the configured `workspace_root`.
/// Wider mirror of `execution::resolve_working_dir`; we duplicate for now to
/// avoid making that fn `pub`.
fn resolve_working_dir(
    requested: Option<&str>,
    workspace_root: &std::path::Path,
) -> Result<std::path::PathBuf, (StatusCode, String)> {
    let requested = match requested {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => return Ok(workspace_root.to_path_buf()),
    };
    let canonical = requested.canonicalize().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("working_dir does not resolve: {e}"),
        )
    })?;
    if !canonical.starts_with(workspace_root) {
        return Err((
            StatusCode::BAD_REQUEST,
            "working_dir escapes the configured workspace root".to_string(),
        ));
    }
    Ok(canonical)
}

fn make_sse_event(event: AgentEvent, id: u64) -> Event {
    let ev_type = match &event {
        AgentEvent::Token { .. } => "token",
        AgentEvent::ToolCallStart { .. } => "tool_call_start",
        AgentEvent::ToolCallEnd { .. } => "tool_call_end",
        AgentEvent::Done { .. } => "done",
        AgentEvent::Error { .. } => "error",
        AgentEvent::ApprovalRequired { .. } => "approval_required",
        AgentEvent::PlanCreated { .. } => "plan_created",
        AgentEvent::WorkerStarted { .. } => "worker_started",
        AgentEvent::WorkerCompleted { .. } => "worker_completed",
        AgentEvent::BudgetProgress { .. } => "budget_progress",
        AgentEvent::ModelInfo { .. } => "model_info",
        AgentEvent::ToolCallPaused { .. } => "tool_call_paused",
        AgentEvent::ContextInjected { .. } => "context_injected",
        AgentEvent::MemoryRecalled { .. } => "memory_recalled",
        _ => "unknown",
    };
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());
    Event::default().event(ev_type).data(data).id(id.to_string())
}

fn build_lead_agent_config(
    provider: Arc<dyn ModelProvider>,
    model_id: ModelId,
    working_dir: std::path::PathBuf,
) -> AgentConfig {
    let tools = ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(WriteFileTool))
        .register(Box::new(EditFileTool))
        .register(Box::new(GlobTool))
        .register(Box::new(GrepTool))
        .register(Box::new(ShellTool))
        .build();

    AgentConfig {
        provider,
        tools,
        sandbox: build_sandbox(),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: ORCHESTRATOR_SYSTEM_PROMPT.to_string(),
        model: model_id.clone(),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: {
            let table = PricingTable::default_table();
            let prefixed = ModelId::new(format!("anthropic:{}", model_id.as_str()));
            table.get(&prefixed).cloned().unwrap_or(PricingEntry {
                model: model_id,
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: None,
                cache_creation_per_million: None,
            })
        },
        working_dir,
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: SkeletonStrategy::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Plan,
    }
}

/// `POST /orchestrate` — multi-agent orchestrated execution.
pub async fn orchestrate_handler(
    State(state): State<AppState>,
    Json(req): Json<OrchestrateRequest>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let working_dir = resolve_working_dir(req.working_dir.as_deref(), &state.workspace_root)?;

    let provider: Option<Arc<dyn ModelProvider>> = state.mock_provider.clone().or_else(|| {
        state
            .provider_key
            .clone()
            .map(|key| -> Arc<dyn ModelProvider> { Arc::new(AnthropicProvider::new(key)) })
    });

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    match provider {
        None => {
            let _ = tx.try_send(AgentEvent::Error {
                message: "ANTHROPIC_API_KEY is not configured on this server.".to_string(),
            });
        }
        Some(provider) => {
            let model_id = ModelId::new(req.model.as_deref().unwrap_or("claude-sonnet-4-6"));
            let max_workers = req.max_workers.unwrap_or(DEFAULT_MAX_WORKERS);
            let total_budget = req.total_budget_usd;
            let prompt = req.prompt;

            let lead_config = build_lead_agent_config(provider, model_id, working_dir.clone());

            // The agent registry is loaded hierarchically (built-ins, user-level,
            // project-level overrides). For an unknown working_dir this falls
            // back to built-ins only.
            let registry = AgentRegistry::load_hierarchical(&working_dir);

            tokio::spawn(async move {
                let mut orchestrator = Orchestrator::new(lead_config, registry, max_workers);
                if let Some(b) = total_budget {
                    orchestrator = orchestrator.with_total_budget(b);
                }
                if let Err(err) = orchestrator.run(&prompt, tx.clone()).await {
                    let _ = tx
                        .send(AgentEvent::Error {
                            message: format!("orchestrator error: {err}"),
                        })
                        .await;
                }
            });
        }
    }

    let mut event_id = 0u64;
    let sse_stream = stream::unfold(rx, move |mut rx| async move {
        rx.recv().await.map(|event| {
            event_id += 1;
            let sse = make_sse_event(event, event_id);
            (Ok::<Event, Infallible>(sse), rx)
        })
    });

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn make_sse_event_serialises_plan_created() {
        let ev = AgentEvent::PlanCreated {
            goal: "demo".into(),
            subtask_count: 3,
        };
        // We can't easily inspect the SSE Event's body via public API, but
        // we can at least verify construction does not panic and the JSON
        // serialisation we depend on works.
        let _ = make_sse_event(ev.clone(), 1);
        let json = serde_json::to_string(&ev).expect("serialises");
        assert!(json.contains("plan_created"));
        assert!(json.contains("\"goal\":\"demo\""));
    }

    #[test]
    fn make_sse_event_serialises_worker_started() {
        let ev = AgentEvent::WorkerStarted {
            task_id: "t1".into(),
            agent: "researcher".into(),
            description: "look it up".into(),
        };
        let _ = make_sse_event(ev.clone(), 2);
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("worker_started"));
        assert!(json.contains("\"task_id\":\"t1\""));
    }

    #[test]
    fn make_sse_event_serialises_worker_completed() {
        let ev = AgentEvent::WorkerCompleted {
            task_id: "t1".into(),
            agent: "researcher".into(),
            status: "completed".into(),
            cost_usd: 0.01,
        };
        let _ = make_sse_event(ev.clone(), 3);
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("worker_completed"));
    }

    #[test]
    fn make_sse_event_serialises_budget_progress() {
        let ev = AgentEvent::BudgetProgress {
            spent_usd: 0.05,
            budget_usd: Some(1.0),
            workers_completed: 2,
            workers_total: 5,
        };
        let _ = make_sse_event(ev.clone(), 4);
        let json = serde_json::to_string(&ev).expect("ser");
        assert!(json.contains("budget_progress"));
        assert!(json.contains("\"workers_completed\":2"));
    }
}
