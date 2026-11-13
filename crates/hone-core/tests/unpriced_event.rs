#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U11 of `docs/plans/usage-and-pricing-spec.md`, agent half: the agent sends
//! `AgentEvent::UnpricedModel { model: String }` the first time a model is unpriced in a
//! session (once per model), so a surface can show its note without asking the agent after
//! `Done`. Priced models never send it.

use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    TokenUsage, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use std::pin::Pin;
use tokio::sync::mpsc;

struct Script;

#[async_trait]
impl ModelProvider for Script {
    fn id(&self) -> &str {
        "script"
    }
    fn display_name(&self) -> &str {
        "Script"
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
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::Token { text: "ok".to_owned() },
            ProviderEvent::Usage(TokenUsage { input: 100, output: 10, cache_read: 0, cache_creation: 0 }),
            ProviderEvent::Done,
        ])))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn lookup() -> PriceLookup {
    Arc::new(|m: &ModelId| {
        (m.as_str() == "mock:a").then(|| PricingEntry {
            model: m.clone(),
            input_per_million: 1.0,
            output_per_million: 1.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        })
    })
}

async fn handle() -> AgentHandle {
    let h = AgentHandle::spawn(AgentConfig {
        provider: Arc::new(Script),
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:a"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:a"),
            input_per_million: 0.0,
            output_per_million: 0.0,
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
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    });
    h.set_price_lookup(lookup()).await;
    h
}

async fn turn_events(h: &AgentHandle) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    h.run_turn("hello", tx).await.expect("turn");
    let mut all = Vec::new();
    while let Ok(e) = rx.try_recv() {
        all.push(e);
    }
    all
}

fn unpriced(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::UnpricedModel { model } => Some(model.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn test_u11_an_unpriced_model_should_send_the_event_once_per_model() {
    let h = handle().await;
    h.set_model(ModelId::new("ollama:llama3")).await;

    let first = turn_events(&h).await;
    let second = turn_events(&h).await;
    h.set_model(ModelId::new("openai:other")).await;
    let third = turn_events(&h).await;
    h.set_model(ModelId::new("ollama:llama3")).await;
    let fourth = turn_events(&h).await;

    assert_eq!(unpriced(&first), ["ollama:llama3"]);
    assert!(unpriced(&second).is_empty(), "already announced");
    assert_eq!(unpriced(&third), ["openai:other"]);
    assert!(unpriced(&fourth).is_empty(), "already announced");
}

#[tokio::test]
async fn test_u11_a_priced_model_should_not_send_the_event() {
    let h = handle().await;

    let events = turn_events(&h).await;

    assert!(unpriced(&events).is_empty(), "{events:?}");
}
