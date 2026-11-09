#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R9a (docs/plans/tui-approval-fix-spec.md section 7): the session's spend must include model
//! calls of turns that were later cancelled.
//!
//! API assumed: `AgentHandle::session_spend(&self) -> (TokenUsage, f64)` (one actor round-trip;
//! `(TokenUsage::default(), 0.0)` if the actor is gone).

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentError, AgentHandle, AgentMode, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, TokenUsage, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::{Value, json};
use tokio::sync::mpsc;

struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "mock_write"
    }
    fn description(&self) -> &str {
        "write"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(json!({ "ok": true })))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn is_read_only(&self) -> bool {
        false
    }
}

/// Every call reports 100 input + 50 output tokens. Call 1 asks for a write; later calls answer.
struct Metered {
    n: AtomicUsize,
}

#[async_trait]
impl ModelProvider for Metered {
    fn id(&self) -> &str {
        "metered"
    }
    fn display_name(&self) -> &str {
        "Metered"
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
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let usage = ProviderEvent::Usage(TokenUsage {
            input: 100,
            output: 50,
            cache_read: 0,
            cache_creation: 0,
        });
        let events = if i == 0 {
            vec![
                usage,
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "c1".to_owned(),
                    name: "mock_write".to_owned(),
                    arguments: json!({}),
                }),
                ProviderEvent::Done,
            ]
        } else {
            vec![
                usage,
                ProviderEvent::Token {
                    text: "done".to_owned(),
                },
                ProviderEvent::Done,
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn handle_with_prompting_agent() -> (AgentHandle, mpsc::Sender<StepAction>) {
    let (step_tx, step_rx) = mpsc::channel::<StepAction>(8);
    let config = AgentConfig {
        provider: Arc::new(Metered {
            n: AtomicUsize::new(0),
        }),
        tools: ToolRegistry::builder().register(Box::new(WriteTool)).build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::env::temp_dir(),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        step_rx: Some(Arc::new(tokio::sync::Mutex::new(step_rx))),
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    };
    (AgentHandle::spawn(config), step_tx)
}

#[tokio::test]
async fn test_r9a_session_spend_when_nothing_ran_should_be_zero() {
    let (handle, _tx) = handle_with_prompting_agent();

    let (usage, cost) = tokio::time::timeout(Duration::from_secs(5), handle.session_spend())
        .await
        .expect("session_spend hung");

    assert_eq!(usage.total(), 0);
    assert_eq!(cost, 0.0);
}

#[tokio::test]
async fn test_r9a_session_spend_should_include_model_call_of_cancelled_turn() {
    let (handle, _step_tx) = handle_with_prompting_agent();
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
    let h = handle.clone();
    let turn = tokio::spawn(async move { h.run_turn("go", tx).await });

    // One model call happens, then the write pauses for approval.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                return;
            }
        }
        panic!("stream ended before a pause");
    })
    .await
    .expect("never paused");
    drop(rx); // client went away

    let res = tokio::time::timeout(Duration::from_secs(10), turn)
        .await
        .expect("turn did not cancel")
        .unwrap();
    assert!(matches!(res, Err(AgentError::Cancelled)), "got {res:?}");

    let (usage, cost) = tokio::time::timeout(Duration::from_secs(5), handle.session_spend())
        .await
        .expect("session_spend hung");

    assert_eq!(usage.input, 100);
    assert_eq!(usage.output, 50);
    let expected = 100.0 * 3.0 / 1e6 + 50.0 * 15.0 / 1e6;
    assert!((cost - expected).abs() < 1e-9, "cost {cost} != {expected}");
}
