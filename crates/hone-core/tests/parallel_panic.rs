/// Fix 3: panicking tools in parallel dispatch must not silently drop results.
///
/// A panic inside a spawned task produces a `JoinError` from `JoinSet::join_next`.
/// The old code silently ignored JoinErrors, leaving the result vec shorter than
/// the call batch and making it impossible to attribute which call panicked.
///
/// The fix converts panics to `AgentError::Internal` and emits a synthetic
/// `ToolCallEnd { is_error: true }` so callers and the event stream both see it.
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::json;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------------

/// A read-only tool that echoes back the `msg` field.
/// Registered under a read-only name and explicitly declares `is_read_only`
/// so the agent batches it with other read-only calls in parallel dispatch.
struct ReadOnlyEcho;

#[async_trait]
impl Tool for ReadOnlyEcho {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Returns the msg field."
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": { "msg": { "type": "string" }, "path": { "type": "string" } } })
    }
    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(
            json!({ "output": input.0.get("msg").cloned().unwrap_or(json!("ok")) }),
        ))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

/// A read-only tool that panics unconditionally.
/// Declares `is_read_only` so it is included in the parallel dispatch batch.
struct PanicTool;

#[async_trait]
impl Tool for PanicTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Always panics."
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": { "pattern": { "type": "string" } } })
    }
    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        panic!("boom from PanicTool");
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Minimal provider that emits parallel tool calls then wraps up
// ---------------------------------------------------------------------------

/// Emits two grep + one glob call (all read-only) in the first response,
/// then a final text token in the second response.
struct ParallelPanicProvider {
    call_count: Mutex<usize>,
}

impl ParallelPanicProvider {
    fn new() -> Self {
        Self { call_count: Mutex::new(0) }
    }
}

#[async_trait]
impl ModelProvider for ParallelPanicProvider {
    fn id(&self) -> &str {
        "parallel-panic-mock"
    }
    fn display_name(&self) -> &str {
        "Parallel Panic Mock"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut c = self.call_count.lock().unwrap();
            let i = *c;
            *c += 1;
            i
        };

        let events = if idx == 0 {
            // First call: emit three parallel read-only tool calls.
            // grep(a), glob (panics), grep(b)
            vec![
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "tc-grep-a".to_owned(),
                    name: "grep".to_owned(),
                    arguments: json!({ "msg": "hello", "path": "/tmp" }),
                }),
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "tc-glob".to_owned(),
                    name: "glob".to_owned(),
                    arguments: json!({ "pattern": "**/*" }),
                }),
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "tc-grep-b".to_owned(),
                    name: "grep".to_owned(),
                    arguments: json!({ "msg": "world", "path": "/tmp" }),
                }),
                ProviderEvent::Done,
            ]
        } else {
            // Follow-up: just finish.
            vec![
                ProviderEvent::Token { text: "done".to_owned() },
                ProviderEvent::Done,
            ]
        };

        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, _: &[Message]) -> u32 {
        5
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Builds an AgentConfig suitable for parallel-panic tests.
fn make_config() -> AgentConfig {
    let registry = ToolRegistry::builder()
        .register(Box::new(ReadOnlyEcho))
        .register(Box::new(PanicTool))
        .build();

    AgentConfig {
        provider: Arc::new(ParallelPanicProvider::new()),
        tools: registry,
        sandbox: Arc::new(NullSandbox),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("/tmp"),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        step_mode: false,
        step_rx: None,
    }
}

/// When one of three parallel tool calls panics, the agent must not lose the
/// other two results and must emit a `ToolCallEnd { is_error: true }` for the
/// panicked call.
#[tokio::test(flavor = "multi_thread")]
async fn test_panic_in_parallel_tool_does_not_drop_results() {
    let mut agent = Agent::new(make_config());
    let (tx, mut rx) = mpsc::channel(128);

    // The agent should complete the turn even though one tool panics.
    let _result = agent
        .run_turn("call parallel tools", tx)
        .await
        .expect("agent should survive a panicking parallel tool");

    // Collect all events.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // There must be a ToolCallEnd with is_error=true for the panicking glob call.
    let panic_end = events.iter().find(|e| {
        matches!(
            e,
            AgentEvent::ToolCallEnd { id, is_error, .. }
            if id == "tc-glob" && *is_error
        )
    });
    assert!(
        panic_end.is_some(),
        "expected ToolCallEnd {{ is_error: true }} for tc-glob, events: {events:#?}"
    );

    // The two grep calls must also have ToolCallEnd events (non-error).
    let grep_a_end = events.iter().find(|e| {
        matches!(
            e,
            AgentEvent::ToolCallEnd { id, is_error, .. }
            if id == "tc-grep-a" && !*is_error
        )
    });
    let grep_b_end = events.iter().find(|e| {
        matches!(
            e,
            AgentEvent::ToolCallEnd { id, is_error, .. }
            if id == "tc-grep-b" && !*is_error
        )
    });
    assert!(grep_a_end.is_some(), "expected successful ToolCallEnd for tc-grep-a");
    assert!(grep_b_end.is_some(), "expected successful ToolCallEnd for tc-grep-b");
}
