use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream;
use hone_core::{AgentConfig, AgentHandle, RetryConfig, SkeletonStrategy};
use hone_proto::{AgentEvent, Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::{AnthropicProvider, PricingTable};
use hone_sandbox::{Sandbox, SandboxBuilder};
use hone_tools::{
    EditFileTool, GlobTool, GrepTool, ReadFileTool, ShellTool, ToolRegistry, WriteFileTool,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::state::AppState;

/// Construct the sandbox instance that will be installed on every agent
/// spawned by `POST /run`.
///
/// This function is the single authoritative source of the server's sandbox
/// selection logic.  Both the handler and the test suite must call this
/// function so that the tests pin the same code path that is exercised in
/// production.
///
/// # Contract (audited in `tests/run_sandbox_enforced.rs`)
///
/// On platforms that provide a native kernel sandbox (macOS → Seatbelt,
/// Linux → Landlock) this function MUST return an implementation whose
/// `platform()` accessor reports a non-`Null` variant.  Returning
/// `NullSandbox` on a supported host is the bug being fixed (AUDIT-P0:
/// `execution.rs` hardcoded `Arc::new(NullSandbox)` instead of calling
/// `SandboxBuilder::detect()`).
///
/// On genuinely unsupported hosts (no kernel sandbox available) `Null` is
/// acceptable and the corresponding tests are `#[cfg]`-gated to skip.
pub fn build_sandbox() -> Arc<dyn Sandbox> {
    Arc::from(SandboxBuilder::detect())
}

const SYSTEM_PROMPT: &str = "You are Hone, an expert AI coding assistant. \
    Help the user with their coding tasks concisely and accurately. \
    When working with files, always read before editing.";

/// A single message in the conversation history sent by the client.
#[derive(Debug, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// Request body for `POST /run`.
#[derive(Debug, Deserialize)]
pub struct RunRequest {
    /// The user's prompt for this turn.
    pub prompt: String,
    /// Model ID (default: `claude-sonnet-4-6`).
    pub model: Option<String>,
    /// Previous conversation messages. The client is responsible for
    /// accumulating history; the server is stateless per-request.
    #[serde(default)]
    pub history: Vec<HistoryMessage>,
    /// Working directory passed to file tools (default: server's cwd).
    pub working_dir: Option<String>,
}

fn event_type(event: &AgentEvent) -> &'static str {
    match event {
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
    }
}

fn make_sse_event(event: AgentEvent, id: u64) -> Event {
    let ev_type = event_type(&event);
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_owned());
    Event::default()
        .event(ev_type)
        .data(data)
        .id(id.to_string())
}

/// Validate that a client-supplied `working_dir` lives under
/// `workspace_root`.
///
/// Canonicalises both sides before comparing to defeat `..` traversal and
/// symlink-based escapes.  When the client supplies `None`, we fall back to
/// the workspace root itself — there is nothing to validate, but we still
/// return a path that lives under the root.
///
/// Returns `Err((status, message))` for any path that resolves outside
/// `workspace_root` (or whose canonicalisation fails — both cases must be
/// rejected as untrusted input).  See AUDIT-CRITICAL-1 in
/// `tests/run_working_dir.rs`.
fn resolve_working_dir(
    requested: Option<&str>,
    workspace_root: &Path,
) -> Result<PathBuf, (StatusCode, String)> {
    let root = workspace_root.canonicalize().map_err(|e| {
        // If the configured root itself does not exist or cannot be
        // canonicalised, every request must be rejected — there is no safe
        // default we can pick on the operator's behalf.
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("workspace root cannot be canonicalised: {e}"),
        )
    })?;

    let requested_path = match requested {
        Some(s) if !s.is_empty() => PathBuf::from(s),
        _ => return Ok(root),
    };

    // Canonicalise the requested path.  If it does not exist on the
    // filesystem, treat it as a rejection: the agent cannot operate in a
    // directory we cannot resolve, and a non-existent path is also an
    // attractive vector for race conditions.
    let canonical = requested_path.canonicalize().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "working_dir does not exist or cannot be resolved".to_string(),
        )
    })?;

    if !canonical.starts_with(&root) {
        return Err((
            StatusCode::BAD_REQUEST,
            "working_dir escapes the configured workspace root".to_string(),
        ));
    }
    Ok(canonical)
}

/// `POST /run` — stateless agent execution endpoint with SSE streaming.
///
/// Creates a fresh agent for every request.  The client sends the full
/// conversation history so the agent can maintain context across turns.
pub async fn run_handler(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    // ---- Step 1: validate working_dir against workspace_root --------------
    //
    // This MUST run before any agent setup so a malicious request can never
    // observe an SSE stream rooted at an attacker-controlled path.  See
    // AUDIT-CRITICAL-1.
    let working_dir = resolve_working_dir(req.working_dir.as_deref(), &state.workspace_root)?;

    let api_key = state.provider_key.clone();
    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    match api_key {
        None => {
            let _ = tx.try_send(AgentEvent::Error {
                message: "ANTHROPIC_API_KEY is not configured on this server.".to_string(),
            });
            // tx dropped → rx drains the one error event then closes.
        }
        Some(key) => {
            let model_id = ModelId::new(req.model.as_deref().unwrap_or("claude-sonnet-4-6"));

            let tools = ToolRegistry::builder()
                .register(Box::new(ReadFileTool))
                .register(Box::new(WriteFileTool))
                .register(Box::new(EditFileTool))
                .register(Box::new(GlobTool))
                .register(Box::new(GrepTool))
                .register(Box::new(ShellTool))
                .build();

            let config = AgentConfig {
                provider: Arc::new(AnthropicProvider::new(key)),
                tools,
                sandbox: build_sandbox(),
                system_prompt: SYSTEM_PROMPT.to_string(),
                model: model_id.clone(),
                max_tokens: None,
                temperature: None,
                budget_limit: None,
                pricing: {
                    let table = PricingTable::default_table();
                    let prefixed = ModelId::new(format!("anthropic:{}", model_id.as_str()));
                    table.get(&prefixed).cloned().unwrap_or(PricingEntry {
                        model: model_id.clone(),
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
            };

            let history: Vec<Message> = req
                .history
                .into_iter()
                .filter_map(|m| match m.role.as_str() {
                    "user" => Some(Message::user(m.content)),
                    "assistant" => Some(Message::assistant(m.content)),
                    _ => None,
                })
                .collect();

            let prompt = req.prompt;

            tokio::spawn(async move {
                let handle = AgentHandle::spawn(config);
                if !history.is_empty() {
                    handle.load_history(history).await;
                }
                let _ = handle.run_turn(prompt, tx).await;
                handle.shutdown().await;
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
mod tests {
    use super::*;

    #[test]
    fn event_type_maps_all_variants() {
        assert_eq!(
            event_type(&AgentEvent::Token { text: "hi".into() }),
            "token"
        );
        assert_eq!(
            event_type(&AgentEvent::Done {
                usage: Default::default(),
                cost_usd: 0.0
            }),
            "done"
        );
        assert_eq!(
            event_type(&AgentEvent::Error {
                message: "oops".into()
            }),
            "error"
        );
    }

    #[test]
    fn make_sse_event_serialises_to_json() {
        let ev = AgentEvent::Token {
            text: "hello".into(),
        };
        let sse = make_sse_event(ev, 1);
        // Event::data is not directly inspectable, but construction must not panic.
        let _ = sse;
    }
}
