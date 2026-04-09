use std::sync::Arc;

use futures::StreamExt;
use hone_memory::{Memory, MemoryQuery, MemoryStore};
use hone_proto::{AgentEvent, Message, ModelId, PricingEntry, ProviderEvent, ToolCallRequest, TokenUsage};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::Sandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinSet;

use crate::context::ContextManager;
use crate::cost::CostAccumulator;
use crate::error::AgentError;
use crate::metrics::{ContextMetrics, MetricsSnapshot};
use crate::recovery::{FailureScenario, RecoveryAction};
use crate::retry::{RetryConfig, is_transient_error, retry};
use crate::turn::TurnResult;

/// Configuration for creating an `Agent`.
#[derive(Clone)]
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
    /// Optional persistent memory store. When set, relevant memories are
    /// injected into the system prompt each turn, and the Q&A pair is recorded
    /// after the turn completes.
    pub memory_store: Option<Arc<Mutex<dyn MemoryStore>>>,
    /// Retry policy applied to transient provider errors (network timeouts,
    /// 429s, 502/503s) before surfacing the error to the model.
    /// Default: 3 attempts with exponential backoff starting at 500 ms.
    pub retry_config: RetryConfig,
}

/// The orchestration core: manages the agent loop, context, tool dispatch, and cost tracking.
pub struct Agent {
    config: AgentConfig,
    context: ContextManager,
    cost: CostAccumulator,
    /// Monotonically increasing counter of completed turns this session.
    turn_count: u32,
    metrics: Arc<ContextMetrics>,
}

impl Agent {
    pub fn new(config: AgentConfig) -> Self {
        let metrics = Arc::new(ContextMetrics::new());
        let context = ContextManager::new(128_000)
            .with_metrics(Arc::clone(&metrics));
        Self {
            config,
            context,
            cost: CostAccumulator::new(),
            turn_count: 0,
            metrics,
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

    /// Estimated token count of the current context window (stored messages
    /// only; the system prompt is not counted here).
    pub fn context_token_count(&self) -> u32 {
        self.context.total_tokens()
    }

    /// The configured maximum context token budget for this agent.
    pub fn context_token_limit(&self) -> u32 {
        self.config.max_tokens.unwrap_or(128_000)
    }

    /// Shared reference to the session metrics accumulator.
    pub fn metrics(&self) -> &Arc<ContextMetrics> {
        &self.metrics
    }

    /// Snapshot of all context management metrics accumulated so far.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Query the memory store for entries relevant to `query` and format them
    /// as an appendix for the system prompt.  Returns an empty string when no
    /// store is configured or no relevant memories exist.
    async fn fetch_relevant_memories(&self, query: &str) -> String {
        let Some(store) = &self.config.memory_store else {
            return String::new();
        };

        let memory_query = MemoryQuery {
            text: query.to_string(),
            limit: 5,
        };

        match store.lock().await.recall(&memory_query).await {
            Ok(memories) if !memories.is_empty() => {
                let mut prompt = String::from("\n\n## Relevant context from memory:\n");
                for mem in &memories {
                    prompt.push_str(&format!("- {}\n", mem.content));
                }
                prompt
            }
            _ => String::new(),
        }
    }

    /// Record the Q&A pair as a new memory entry (fire-and-forget; errors are
    /// silently ignored so a store failure never breaks the agent loop).
    async fn record_memory(&self, user_input: &str, response_text: &str) {
        let Some(store) = &self.config.memory_store else {
            return;
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();

        // Generate a unique ID from timestamp nanos + a thread-local counter.
        let id = {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
            format!("{:016x}-{:08x}", now.as_nanos() as u64, seq)
        };

        let memory = Memory {
            id,
            content: format!("Q: {user_input}\nA: {response_text}"),
            tags: vec![],
            weight: 1.0,
            created_at: now.as_secs(),
            decay_at: None,
        };

        let _ = store.lock().await.record(memory).await;
    }

    /// Run a single conversation turn: stream tokens, dispatch any tool calls,
    /// and return a `TurnResult`.
    ///
    /// Events are sent on `events` as they arrive; the caller may drop the
    /// receiver at any time without causing this function to fail.
    #[tracing::instrument(skip(self, events), fields(
        model = %self.config.model,
        turn = tracing::field::Empty
    ))]
    pub async fn run_turn(
        &mut self,
        user_input: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TurnResult, AgentError> {
        self.turn_count += 1;
        tracing::Span::current().record("turn", self.turn_count);
        tracing::debug!(turn_number = self.turn_count, "starting turn");

        self.cost.check_budget(self.config.budget_limit)?;

        // Fetch memories before adding the user message so the query is clean.
        let memories = self.fetch_relevant_memories(user_input).await;

        self.context.add_message(Message::user(user_input));

        let mut response_text = String::new();
        let mut turn_usage = TokenUsage::default();
        let mut tool_calls_count: u32 = 0;

        // We loop so that tool call → model follow-up cycles are handled
        // transparently.
        loop {
            // Build the base message list with the STABLE system prompt.
            // Memories are injected as a separate system-role message right
            // after the base system prompt, so the prefix stays cache-friendly.
            let mut messages = self
                .context
                .build_messages(&self.config.system_prompt);

            if !memories.is_empty() {
                let memory_msg = Message {
                    role: hone_proto::Role::System,
                    content: hone_proto::Content::Text(format!(
                        "# Relevant Memories\n\n{}",
                        memories
                    )),
                    tool_calls: vec![],
                    tool_call_id: None,
                };
                // Insert right after the base system prompt (always index 0).
                if messages.first().is_some_and(|m| matches!(m.role, hone_proto::Role::System)) {
                    messages.insert(1, memory_msg);
                } else {
                    messages.insert(0, memory_msg);
                }
            }

            let mut request = CompletionRequest::new(self.config.model.clone(), messages);
            request.temperature = self.config.temperature;
            request.max_tokens = self.config.max_tokens;
            request.tools = self.config.tools.schemas();

            let retry_cfg = self.config.retry_config.clone();
            let provider = Arc::clone(&self.config.provider);
            let request_clone = request.clone();
            let mut stream = retry(
                &retry_cfg,
                || {
                    let req = request_clone.clone();
                    let prov = Arc::clone(&provider);
                    async move { prov.complete(req).await }
                },
                |e: &ProviderError| is_transient_error(&e.to_string()),
            )
            .await?;

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

            // Dispatch tool calls. Consecutive read-only calls are executed in
            // parallel via a JoinSet to reduce wall-clock time; side-effecting
            // calls are always dispatched sequentially to preserve safety.
            let ctx = ToolContext {
                sandbox: Arc::clone(&self.config.sandbox),
                working_dir: self.config.working_dir.clone(),
            };

            let mut i = 0;
            while i < pending_tool_calls.len() {
                if is_read_only_tool(&pending_tool_calls[i].name) {
                    // Find the end of the consecutive read-only run.
                    let mut j = i;
                    while j < pending_tool_calls.len()
                        && is_read_only_tool(&pending_tool_calls[j].name)
                    {
                        j += 1;
                    }

                    // Execute [i..j] in parallel and collect results in order.
                    let batch = pending_tool_calls[i..j].to_vec();
                    let batch_results = dispatch_parallel(
                        &self.config.tools,
                        &ctx,
                        batch.clone(),
                        &events,
                    )
                    .await;

                    // Consume both vecs by value to avoid needing Clone on AgentError.
                    for (call, result) in batch.into_iter().zip(batch_results.into_iter()) {
                        tool_calls_count += 1;
                        tracing::debug!(tool_name = %call.name, "tool dispatched (parallel)");

                        match result {
                            Ok(value) => {
                                self.context.add_message(Message::tool_result(
                                    call.id.clone(),
                                    serde_json::to_string(&value).unwrap_or_default(),
                                ));
                            }
                            Err(AgentError::ToolNotFound { name }) => {
                                let err_val = serde_json::json!({
                                    "error": format!("tool '{name}' not found")
                                });
                                self.context.add_message(Message::tool_result(
                                    call.id.clone(),
                                    serde_json::to_string(&err_val).unwrap_or_default(),
                                ));
                            }
                            // Fatal errors are returned immediately with ownership.
                            Err(e) => return Err(e),
                        }
                    }

                    i = j;
                } else {
                    // Side-effecting call: dispatch sequentially.
                    let call = &pending_tool_calls[i];
                    tracing::debug!(tool_name = %call.name, "tool dispatched (sequential)");
                    let _ = events
                        .send(AgentEvent::ToolCallStart {
                            id: call.id.clone(),
                            name: call.name.clone(),
                        })
                        .await;

                    tool_calls_count += 1;

                    let tool = self.config.tools.get(&call.name).map(Arc::clone);
                    let result = match tool {
                        None => Err(AgentError::ToolNotFound {
                            name: call.name.clone(),
                        }),
                        Some(t) => dispatch_with_recovery(
                            t,
                            ToolInput::new(call.arguments.clone()),
                            &ctx,
                        )
                        .await
                        .map_err(|e| AgentError::Tool {
                            name: call.name.clone(),
                            source: e,
                        }),
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
                            let err_val = serde_json::json!({
                                "error": format!("tool '{name}' not found")
                            });
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

                    i += 1;
                }
            }

            // Loop: call provider again with the updated context (tool results
            // now included).
        }

        let cost_usd = self.cost.total_usd();
        tracing::debug!(
            tokens = turn_usage.total(),
            tool_calls = tool_calls_count,
            cost_usd,
            "turn completed"
        );

        // Record per-turn token usage in metrics.
        self.metrics.record_turn_tokens(turn_usage.input, turn_usage.output);
        // Record cache hits (cache_read maps to Anthropic's cache_read_input_tokens).
        // Cache writes are not yet tracked separately in TokenUsage.
        if turn_usage.cache_read > 0 {
            self.metrics.record_cache(turn_usage.cache_read, 0);
        }

        // Persist Q&A pair to memory (fire-and-forget).
        self.record_memory(user_input, &response_text).await;

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

// ---------------------------------------------------------------------------
// Recovery-aware sequential tool dispatch
// ---------------------------------------------------------------------------

/// Dispatch a single tool call with one automatic recovery attempt for
/// classified transient failures.
///
/// On the first error, the failure is classified by [`FailureScenario`].  If
/// the recommended action is [`RecoveryAction::RetryAfter`], the call sleeps
/// for the specified duration and retries exactly once.  All other scenarios
/// (Escalate, RunCommandThenRetry, GiveUp) fall through immediately so the
/// error is fed to the model.
async fn dispatch_with_recovery(
    tool: Arc<dyn Tool>,
    input: ToolInput,
    ctx: &ToolContext,
) -> Result<ToolOutput, ToolError> {
    match tool.call(input.clone(), ctx).await {
        Ok(output) => Ok(output),
        Err(e) => {
            let scenario = FailureScenario::classify(&e.to_string());
            if let RecoveryAction::RetryAfter(duration) = scenario.recommended_action() {
                tracing::info!(
                    scenario = ?scenario,
                    delay_ms = duration.as_millis(),
                    "auto-retrying failed tool after classified recovery"
                );
                tokio::time::sleep(duration).await;
                tool.call(input, ctx).await
            } else {
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parallel tool dispatch helpers
// ---------------------------------------------------------------------------

/// Returns `true` for tools that are safe to execute concurrently because they
/// are read-only and produce no observable side-effects on the filesystem, the
/// repository, or any external system.
pub fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "glob"
            | "grep"
            | "git_status"
            | "git_diff"
            | "lsp_diagnostics"
            | "lsp_hover"
            | "lsp_code_actions"
            | "symbol_search"
            | "symbol_at"
    )
}

/// Dispatch a batch of tool calls in parallel via a [`JoinSet`].
///
/// Results are returned in the **same order** as `calls`, regardless of
/// completion order.  Each call emits `ToolCallStart` / `ToolCallEnd` events
/// on `events`.
async fn dispatch_parallel(
    registry: &ToolRegistry,
    ctx: &ToolContext,
    calls: Vec<ToolCallRequest>,
    events: &mpsc::Sender<AgentEvent>,
) -> Vec<Result<Value, AgentError>> {
    let mut join_set: JoinSet<(usize, Result<Value, AgentError>)> = JoinSet::new();

    for (idx, call) in calls.into_iter().enumerate() {
        let tool = registry.get(&call.name).map(Arc::clone);
        let ctx_clone = ctx.clone();
        let events_clone = events.clone();

        join_set.spawn(async move {
            let _ = events_clone
                .send(AgentEvent::ToolCallStart {
                    id: call.id.clone(),
                    name: call.name.clone(),
                })
                .await;

            let result: Result<Value, AgentError> = match tool {
                None => Err(AgentError::ToolNotFound {
                    name: call.name.clone(),
                }),
                Some(t) => t
                    .call(ToolInput::new(call.arguments.clone()), &ctx_clone)
                    .await
                    .map(|o| o.into_value())
                    .map_err(|e| AgentError::Tool {
                        name: call.name.clone(),
                        source: e,
                    }),
            };

            let is_error = result.is_err();
            let result_value = match &result {
                Ok(v) => v.clone(),
                Err(AgentError::ToolNotFound { name }) => {
                    serde_json::json!({ "error": format!("tool '{name}' not found") })
                }
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            };

            let _ = events_clone
                .send(AgentEvent::ToolCallEnd {
                    id: call.id.clone(),
                    result: result_value,
                    is_error,
                })
                .await;

            (idx, result)
        });
    }

    // Collect completions in any order, then sort by original index.
    let mut indexed: Vec<(usize, Result<Value, AgentError>)> =
        Vec::with_capacity(join_set.len());
    while let Some(joined) = join_set.join_next().await {
        if let Ok(pair) = joined {
            indexed.push(pair);
        }
    }
    indexed.sort_by_key(|(idx, _)| *idx);
    indexed.into_iter().map(|(_, r)| r).collect()
}

// ---------------------------------------------------------------------------
// Actor pattern: AgentHandle
// ---------------------------------------------------------------------------

/// Commands sent through the agent actor's command channel.
enum AgentCommand {
    RunTurn {
        user_input: String,
        events: mpsc::Sender<AgentEvent>,
        reply: oneshot::Sender<Result<TurnResult, AgentError>>,
    },
    SwapProvider {
        provider: Arc<dyn ModelProvider>,
    },
    GetCost {
        reply: oneshot::Sender<f64>,
    },
    GetContextTokens {
        reply: oneshot::Sender<u32>,
    },
    Shutdown,
}

/// A cheap-to-clone handle to a background agent actor task.
///
/// All calls are serialized through a command channel so no `Mutex` is needed
/// at the call site.  The actor owns the `Agent` exclusively and processes one
/// command at a time.
#[derive(Clone)]
pub struct AgentHandle {
    cmd_tx: mpsc::Sender<AgentCommand>,
}

impl AgentHandle {
    /// Spawn an agent actor task and return a handle to it.
    pub fn spawn(config: AgentConfig) -> Self {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(32);

        tokio::spawn(async move {
            let mut agent = Agent::new(config);
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AgentCommand::RunTurn {
                        user_input,
                        events,
                        reply,
                    } => {
                        let result = agent.run_turn(&user_input, events).await;
                        let _ = reply.send(result);
                    }
                    AgentCommand::SwapProvider { provider } => {
                        agent.swap_provider(provider);
                    }
                    AgentCommand::GetCost { reply } => {
                        let _ = reply.send(agent.session_cost());
                    }
                    AgentCommand::GetContextTokens { reply } => {
                        let _ = reply.send(agent.context_token_count());
                    }
                    AgentCommand::Shutdown => break,
                }
            }
        });

        Self { cmd_tx }
    }

    /// Run a single conversation turn.  Awaits until the actor has finished
    /// processing the turn and returns the result.
    pub async fn run_turn(
        &self,
        user_input: impl Into<String>,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<TurnResult, AgentError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(AgentCommand::RunTurn {
                user_input: user_input.into(),
                events,
                reply: reply_tx,
            })
            .await
            .map_err(|_| AgentError::Cancelled)?;

        reply_rx.await.map_err(|_| AgentError::Cancelled)?
    }

    /// Hot-swap the model provider used by the actor.
    pub async fn swap_provider(
        &self,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), AgentError> {
        self.cmd_tx
            .send(AgentCommand::SwapProvider { provider })
            .await
            .map_err(|_| AgentError::Cancelled)
    }

    /// Return the total USD cost accumulated across all turns so far.
    pub async fn session_cost(&self) -> f64 {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(AgentCommand::GetCost { reply: tx }).await.is_err() {
            return 0.0;
        }
        rx.await.unwrap_or(0.0)
    }

    /// Return the estimated token count of the current context window.
    pub async fn context_tokens(&self) -> u32 {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx
            .send(AgentCommand::GetContextTokens { reply: tx })
            .await
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    /// Signal the actor to shut down.  The actor will finish processing any
    /// in-flight command before terminating.
    pub async fn shutdown(self) {
        let _ = self.cmd_tx.send(AgentCommand::Shutdown).await;
    }
}
