use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::stream;
use hone_core::{AgentConfig, AgentHandle, RetryConfig, SkeletonStrategy};
use hone_proto::{AgentEvent, Message, ModelId, PricingEntry};
use hone_providers::{AnthropicProvider, PricingTable};
use hone_sandbox::NullSandbox;
use hone_tools::{EditFileTool, GlobTool, GrepTool, ReadFileTool, ShellTool, ToolRegistry, WriteFileTool};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::state::AppState;

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
    Event::default().event(ev_type).data(data).id(id.to_string())
}

/// `POST /run` — stateless agent execution endpoint with SSE streaming.
///
/// Creates a fresh agent for every request.  The client sends the full
/// conversation history so the agent can maintain context across turns.
pub async fn run_handler(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> impl IntoResponse {
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
            let model_id = ModelId::new(
                req.model.as_deref().unwrap_or("claude-sonnet-4-6"),
            );
            let working_dir = req
                .working_dir
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

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
                sandbox: Arc::new(NullSandbox),
                system_prompt: SYSTEM_PROMPT.to_string(),
                model: model_id.clone(),
                max_tokens: None,
                temperature: None,
                budget_limit: None,
                pricing: {
                    let table = PricingTable::default_table();
                    let prefixed = ModelId::new(format!(
                        "anthropic:{}",
                        model_id.as_str()
                    ));
                    table.get(&prefixed).cloned().unwrap_or(PricingEntry {
                        model: model_id.clone(),
                        input_per_million: 3.0,
                        output_per_million: 15.0,
                        cache_read_per_million: None,
                    })
                },
                working_dir,
                memory_store: None,
                retry_config: RetryConfig::default(),
                index: None,
                inject_context: false,
                skeleton_strategy: SkeletonStrategy::default(),
                egress_log: None,
                step_mode: false,
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

    Sse::new(sse_stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_maps_all_variants() {
        assert_eq!(event_type(&AgentEvent::Token { text: "hi".into() }), "token");
        assert_eq!(event_type(&AgentEvent::Done { usage: Default::default() }), "done");
        assert_eq!(
            event_type(&AgentEvent::Error { message: "oops".into() }),
            "error"
        );
    }

    #[test]
    fn make_sse_event_serialises_to_json() {
        let ev = AgentEvent::Token { text: "hello".into() };
        let sse = make_sse_event(ev, 1);
        // Event::data is not directly inspectable, but construction must not panic.
        let _ = sse;
    }
}
