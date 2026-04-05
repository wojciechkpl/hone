use std::sync::Arc;

use futures::StreamExt;
use hone_proto::{AgentEvent, Message, ModelId, PricingEntry, ProviderEvent, TokenUsage};
use hone_providers::{CompletionRequest, ModelProvider};
use hone_sandbox::Sandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

use crate::context::ContextManager;
use crate::cost::CostAccumulator;
use crate::error::AgentError;
use crate::turn::TurnResult;

/// Configuration for creating an `Agent`.
pub struct AgentConfig {
    pub provider: Arc<dyn ModelProvider>,
    pub tools: ToolRegistry,
    pub sandbox: Arc<dyn Sandbox>,
    pub system_prompt: String,
    pub model: ModelId,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Maximum USD spend permitted for this session. `None` means unlimited.
    pub budget_limit: Option<f64>,
    /// Pricing rates for the current model, used to convert token counts to USD.
    pub pricing: PricingEntry,
    /// Working directory passed to tool calls.
    pub working_dir: std::path::PathBuf,
}

/// The orchestration core: manages the agent loop, context, tool dispatch, and cost tracking.
pub struct Agent {
    config: AgentConfig,
    context: ContextManager,
    cost: CostAccumulator,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            context: ContextManager::new(128_000),
            cost: CostAccumulator::new(),
        }
    }

    /// Swap out the active provider (e.g. for hot-reload / fallback tests).
    pub fn swap_provider(&mut self, provider: Arc<dyn ModelProvider>) {
        self.config.provider = provider;
    }

    /// Total USD cost accumulated across all turns this session.
    pub fn session_cost(&self) -> f64 {
        self.cost.total_usd()
    }

    /// Run a single conversation turn: stream tokens, dispatch any tool calls,
    /// and return a `TurnResult`.
    ///
    /// Events are sent on `events` as they arrive; the caller may drop the
    /// receiver at any time without causing this function to fail.
    pub async fn run_turn(
        &mut self,
        user_input: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TurnResult, AgentError> {
        self.cost.check_budget(self.config.budget_limit)?;

        self.context.add_message(Message::user(user_input));

        let mut response_text = String::new();
        let mut turn_usage = TokenUsage::default();
        let mut tool_calls_count: u32 = 0;

        // We loop so that tool call → model follow-up cycles are handled
        // transparently.
        loop {
            let messages = self
                .context
                .build_messages(&self.config.system_prompt);

            let mut request = CompletionRequest::new(self.config.model.clone(), messages);
            request.temperature = self.config.temperature;
            request.max_tokens = self.config.max_tokens;
            request.tools = self.config.tools.schemas();

            let mut stream = self.config.provider.complete(request).await?;

            // Collect any tool calls from this streaming response so we can
            // process them after the stream is exhausted.
            let mut pending_tool_calls: Vec<hone_proto::ToolCallRequest> = Vec::new();
            let mut assistant_text = String::new();

            while let Some(event) = stream.next().await {
                match event {
                    ProviderEvent::Token { text } => {
                        assistant_text.push_str(&text);
                        response_text.push_str(&text);
                        let _ = events.send(AgentEvent::Token { text }).await;
                    }

                    ProviderEvent::ToolCall(call) => {
                        pending_tool_calls.push(call);
                    }

                    ProviderEvent::Usage(usage) => {
                        turn_usage = turn_usage + usage;
                        self.cost.add(usage, &self.config.pricing);
                    }

                    ProviderEvent::Done => break,

                    ProviderEvent::Error { message } => {
                        let _ = events.send(AgentEvent::Error { message: message.clone() }).await;
                        return Err(AgentError::Provider(
                            hone_providers::ProviderError::Http {
                                provider: self.config.provider.id().to_owned(),
                                status: 0,
                                body: message,
                            },
                        ));
                    }
                }
            }

            if pending_tool_calls.is_empty() {
                // No tool calls → this is the final assistant turn; persist it
                // and return.
                if !assistant_text.is_empty() {
                    self.context.add_message(Message::assistant(assistant_text));
                }
                break;
            }

            // Build an assistant message that carries the tool_calls list so
            // the provider sees the full conversation.
            let assistant_msg = Message {
                role: hone_proto::Role::Assistant,
                content: assistant_text.clone().into(),
                tool_calls: pending_tool_calls.clone(),
                tool_call_id: None,
            };
            self.context.add_message(assistant_msg);

            // Dispatch each tool call and append the results to context.
            for call in &pending_tool_calls {
                let _ = events
                    .send(AgentEvent::ToolCallStart {
                        id: call.id.clone(),
                        name: call.name.clone(),
                    })
                    .await;

                tool_calls_count += 1;

                let tool = self.config.tools.get(&call.name);
                let result = match tool {
                    None => Err(AgentError::ToolNotFound {
                        name: call.name.clone(),
                    }),
                    Some(t) => {
                        let ctx = hone_tools::ToolContext {
                            sandbox: Arc::clone(&self.config.sandbox),
                            working_dir: self.config.working_dir.clone(),
                        };
                        t.call(hone_tools::ToolInput::new(call.arguments.clone()), &ctx)
                            .await
                            .map_err(|e| AgentError::Tool {
                                name: call.name.clone(),
                                source: e,
                            })
                    }
                };

                match result {
                    Ok(output) => {
                        let value = output.into_value();
                        let _ = events
                            .send(AgentEvent::ToolCallEnd {
                                id: call.id.clone(),
                                result: value.clone(),
                                is_error: false,
                            })
                            .await;
                        self.context.add_message(Message::tool_result(
                            call.id.clone(),
                            serde_json::to_string(&value).unwrap_or_default(),
                        ));
                    }
                    Err(AgentError::ToolNotFound { name }) => {
                        let err_val = serde_json::json!({ "error": format!("tool '{name}' not found") });
                        let _ = events
                            .send(AgentEvent::ToolCallEnd {
                                id: call.id.clone(),
                                result: err_val.clone(),
                                is_error: true,
                            })
                            .await;
                        self.context.add_message(Message::tool_result(
                            call.id.clone(),
                            serde_json::to_string(&err_val).unwrap_or_default(),
                        ));
                    }
                    Err(e) => return Err(e),
                }
            }

            // Loop: call provider again with the updated context (tool results
            // now included).
        }

        let cost_usd = self.cost.total_usd();

        let _ = events
            .send(AgentEvent::Done { usage: turn_usage })
            .await;

        Ok(TurnResult {
            response: response_text,
            usage: turn_usage,
            cost_usd,
            tool_calls_count,
        })
    }
}
