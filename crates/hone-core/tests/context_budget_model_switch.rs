#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract (spec F3): the models that may serve a request besides its own
//! come from `ModelProvider::fallback_models(&self, &ModelId) -> Vec<ModelId>`
//! (default: none), and each lowers the request budget to 60% of that model's
//! window (its window only: no Haiku 5.5 cost cap, no other family's ratio).
//! Does not compile until the trait method exists.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, estimate_text_tokens};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn total_tokens(msgs: &[Message]) -> u32 {
    msgs.iter()
        .map(|m| estimate_text_tokens(&m.content.as_text()))
        .sum()
}

/// `n` distinct messages of roughly 6.5k tokens each, starting with a user message.
fn history(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| {
            let text: String = (0..1500).map(|j| format!("w{i}x{j} ")).collect();
            if i % 2 == 0 {
                Message::user(text)
            } else {
                Message::assistant(text)
            }
        })
        .collect()
}

/// About 400K estimated tokens.
const BIG: usize = 62;

/// A mock provider that reports the given fallback models for every request.
struct WithFallbacks {
    inner: Arc<MockProvider>,
    fallbacks: Vec<ModelId>,
}

#[async_trait]
impl ModelProvider for WithFallbacks {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn display_name(&self) -> &str {
        "with-fallbacks"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        self.inner.supported_models()
    }
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        self.inner.complete(request).await
    }
    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.inner.estimate_tokens(messages)
    }
    async fn health(&self) -> ProviderHealth {
        self.inner.health().await
    }
    fn fallback_models(&self, _model: &ModelId) -> Vec<ModelId> {
        self.fallbacks.clone()
    }
}

fn agent_config(model: &str, provider: Arc<dyn ModelProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new(model),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new(model),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("."),
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
    }
}

/// Request tokens for a turn on `anthropic:claude-opus-4-7` holding ~400K of
/// history, when the provider reports `fallbacks`.
async fn request_tokens_with_fallbacks(fallbacks: &[&str]) -> u32 {
    let mock = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let provider = Arc::new(WithFallbacks {
        inner: Arc::clone(&mock),
        fallbacks: fallbacks.iter().map(|m| ModelId::new(*m)).collect(),
    });
    let h = AgentHandle::spawn(agent_config("anthropic:claude-opus-4-7", provider));
    h.load_history(history(BIG)).await;
    let (tx, _rx) = mpsc::channel(64);
    h.run_turn("next question", tx).await.expect("turn");
    let req = mock.last_request.lock().unwrap().clone().expect("request");
    total_tokens(&req.messages)
}

#[tokio::test]
async fn test_f3_request_when_fallback_is_200k_model_should_carry_at_most_120k() {
    let sent = request_tokens_with_fallbacks(&["anthropic:claude-haiku-4-5"]).await;
    assert!(sent <= 120_000, "request carried {sent} tokens (> 120K)");
    assert!(sent > 60_000, "request over-trimmed: {sent} tokens");
}

#[tokio::test]
async fn test_f3_request_when_fallback_is_haiku_5_5_should_use_its_window_not_its_cap() {
    // 60% of the 1M window = 600K: the whole ~400K history is sent.
    let sent = request_tokens_with_fallbacks(&["anthropic:claude-haiku-5-5"]).await;
    assert!(
        sent > 380_000,
        "request carried only {sent} tokens (60K cap leaked)"
    );
}

#[tokio::test]
async fn test_f3_request_when_fallback_is_gpt_4o_should_carry_at_most_76_8k() {
    // 60% of 128K, not gpt-4's family ratio of 0.55.
    let sent = request_tokens_with_fallbacks(&["openai:gpt-4o"]).await;
    assert!(sent <= 76_800, "request carried {sent} tokens (> 76_800)");
    assert!(sent > 60_000, "request over-trimmed: {sent} tokens");
}

#[tokio::test]
async fn test_f3_request_when_several_fallbacks_should_use_the_smallest_limit() {
    let sent = request_tokens_with_fallbacks(&[
        "anthropic:claude-haiku-5-5",
        "anthropic:claude-haiku-4-5",
    ])
    .await;
    assert!(sent <= 120_000, "request carried {sent} tokens (> 120K)");
}

#[tokio::test]
async fn test_f3_request_when_no_fallbacks_should_use_current_model_budget() {
    let sent = request_tokens_with_fallbacks(&[]).await;
    assert!(sent > 380_000, "request carried only {sent} tokens");
}
