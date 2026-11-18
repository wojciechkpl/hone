#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier through a real `FallbackChain` (spec H5a, review round 1).
//!
//! `FallbackChain` emits `ModelFallback` first, then the usage carried over from members that
//! failed before output, then the answering member's own events. A usage event that carries
//! prompt tokens starts a NEW request: the failed member's 60,000-token prompt and the
//! fallback's 60,000-token prompt are two base-rate requests, not one 120,000-token request.
//!
//! Both members are named `anthropic:claude-haiku-5-5`, so the answer does not depend on which
//! of them the failed member's usage is attributed to (pre-existing behaviour, not pinned).
//! The built-in price table is the agent's lookup (`set_price_lookup`).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StreamErrorKind, TokenUsage, ToolApprovalPolicy,
};
use hone_providers::{
    CompletionRequest, FallbackChain, ModelProvider, PricingTable, ProviderError,
};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

const HAIKU: &str = "anthropic:claude-haiku-5-5";

/// A member that answers every call with the same events.
struct Member {
    events: Vec<ProviderEvent>,
}

#[async_trait]
impl ModelProvider for Member {
    fn id(&self) -> &str {
        HAIKU
    }
    fn display_name(&self) -> &str {
        "Member"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new(HAIKU),
            display_name: "Haiku".to_owned(),
            context_length: 1_000_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(self.events.clone())))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn member(events: Vec<ProviderEvent>) -> Arc<dyn ModelProvider> {
    Arc::new(Member { events })
}

fn usage(input: u32, output: u32) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input,
        output,
        cache_read: 0,
        cache_creation: 0,
    })
}

fn fail(kind: StreamErrorKind) -> ProviderEvent {
    ProviderEvent::Error {
        message: "boom".to_owned(),
        kind,
    }
}

async fn session_cost(members: Vec<Arc<dyn ModelProvider>>) -> f64 {
    let h = AgentHandle::spawn(AgentConfig {
        provider: Arc::new(FallbackChain::new(members)),
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new(HAIKU),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new(HAIKU),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
            long_prompt: None,
        },
        working_dir: std::env::temp_dir(),
        memory_store: None,
        retry_config: RetryConfig {
            max_attempts: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            multiplier: 1.0,
        },
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    });
    let table = PricingTable::default_table();
    let lookup: PriceLookup = Arc::new(move |m: &ModelId| table.get(m).cloned());
    h.set_price_lookup(lookup).await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    // The all-members-fail turn ends in an error; the cost counted so far is what matters.
    let _ = tokio::time::timeout(Duration::from_secs(20), h.run_turn("go", tx))
        .await
        .expect("turn hung");
    while rx.try_recv().is_ok() {}
    h.session_spend().await.1
}

#[tokio::test]
async fn test_h5a_a_fallback_after_a_failed_member_should_be_priced_on_its_own_prompt() {
    // Member 1: 60,000-token prompt, then fails before output (retryable).
    // Member 2 answers: 60,000-token prompt, 1,000 output.
    // Each prompt is a base-rate request: 6,000 (failed) + 6,000 + 500 per million.
    let cost = session_cost(vec![
        member(vec![usage(60_000, 0), fail(StreamErrorKind::Unclassified)]),
        member(vec![
            usage(60_000, 0),
            usage(0, 1_000),
            ProviderEvent::Token {
                text: "ok".to_owned(),
            },
            ProviderEvent::Done,
        ]),
    ])
    .await;
    assert!(
        (cost - 0.0125).abs() < 1e-12,
        "two 60K prompts are two base-rate requests (0.0125), not one 120K long request: {cost}"
    );
}

#[tokio::test]
async fn test_h5a_when_every_member_fails_no_event_should_be_priced_at_the_long_rate() {
    // Both members report 60,000 and fail; the last error is not retryable, so the chain
    // delivers both members' usage in one stream. 6,000 + 6,000 per million, all base.
    let cost = session_cost(vec![
        member(vec![usage(60_000, 0), fail(StreamErrorKind::Unclassified)]),
        member(vec![
            usage(60_000, 0),
            fail(StreamErrorKind::Provider { status: Some(400) }),
        ]),
    ])
    .await;
    assert!(
        (cost - 0.012).abs() < 1e-12,
        "two 60K prompts are two base-rate requests (0.012): {cost}"
    );
}
