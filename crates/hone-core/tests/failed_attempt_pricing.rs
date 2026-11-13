#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U16 (guard) of `docs/plans/usage-and-pricing-spec.md`: usage reported by a FAILED attempt is priced by
//! the model that was tried (here a fallback announced in that attempt), not by the session's
//! configured model. This passes today: the scripted provider announces the fallback inside the
//! failing stream, which already counts as output and takes the normal pricing path; the
//! `count_usage` path for attempts that failed before any output only sees `Usage` events, so
//! no scripted stream can show a different model there. Uses the existing `AgentHandle::set_price_lookup` / `session_spend`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StreamErrorKind, TokenUsage, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

/// Every attempt: the fallback `mock:fallback` took over, reported 1M input tokens, then the
/// stream failed.
struct Failing;

#[async_trait]
impl ModelProvider for Failing {
    fn id(&self) -> &str {
        "failing"
    }
    fn display_name(&self) -> &str {
        "Failing"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:a"),
            display_name: "A".to_owned(),
            context_length: 128_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::ModelFallback {
                requested: r.model.as_str().to_owned(),
                fallback: "mock:fallback".to_owned(),
                reason: "test".to_owned(),
            },
            ProviderEvent::Usage(TokenUsage { input: 1_000_000, output: 0, cache_read: 0, cache_creation: 0 }),
            ProviderEvent::Error {
                message: "boom".to_owned(),
                kind: StreamErrorKind::Unclassified,
            },
        ])))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn entry(model: &str, per_million: f64) -> PricingEntry {
    PricingEntry {
        model: ModelId::new(model),
        input_per_million: per_million,
        output_per_million: 0.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

#[tokio::test]
async fn test_u16_usage_of_a_failed_attempt_on_a_fallback_model_should_be_priced_by_that_fallback() {
    let h = AgentHandle::spawn(AgentConfig {
        provider: Arc::new(Failing),
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:a"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: entry("mock:a", 0.0),
        working_dir: std::env::temp_dir(),
        memory_store: None,
        retry_config: RetryConfig {
            max_attempts: 1,
            initial_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(2),
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
    let lookup: PriceLookup = Arc::new(|m: &ModelId| match m.as_str() {
        "mock:a" => Some(entry("mock:a", 1.0)),
        "mock:fallback" => Some(entry("mock:fallback", 100.0)),
        _ => None,
    });
    h.set_price_lookup(lookup).await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let _ = h.run_turn("hello", tx).await; // the turn fails
    while rx.try_recv().is_ok() {}

    let (usage, cost) = h.session_spend().await;
    assert_eq!(usage.input, 1_000_000, "the failed attempt's usage is counted");
    assert!(
        (cost - 100.0).abs() < 1e-9,
        "priced by mock:fallback ($100 per 1M), not by the configured mock:a ($1): {cost}"
    );
}
