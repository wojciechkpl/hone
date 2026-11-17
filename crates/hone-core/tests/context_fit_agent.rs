#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Spec G2, G3 (receipts) and G5 through the agent with a mock provider that
//! reports fallback models, as in `context_budget_model_switch.rs`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_core::{AgentConfig, AgentHandle, AgentMode, RetryConfig, estimate_text_tokens};
use hone_proto::{
    AgentEvent, ContextReceipt, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent,
    ProviderHealth, ToolApprovalPolicy,
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

/// `n` distinct messages of `words` words each (~4.3 tokens per word).
fn history(n: usize, words: usize) -> Vec<Message> {
    (0..n)
        .map(|i| {
            let text: String = (0..words).map(|j| format!("w{i}x{j} ")).collect();
            if i % 2 == 0 {
                Message::user(text)
            } else {
                Message::assistant(text)
            }
        })
        .collect()
}

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

fn rig(model: &str, fallbacks: &[&str]) -> (AgentHandle, Arc<MockProvider>) {
    let mock = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let provider = Arc::new(WithFallbacks {
        inner: Arc::clone(&mock),
        fallbacks: fallbacks.iter().map(|m| ModelId::new(*m)).collect(),
    });
    (AgentHandle::spawn(agent_config(model, provider)), mock)
}

async fn turn(h: &AgentHandle) -> Vec<AgentEvent> {
    let (tx, mut rx) = mpsc::channel(256);
    h.run_turn("next question", tx).await.expect("turn");
    let mut evs = Vec::new();
    while let Ok(e) = rx.try_recv() {
        evs.push(e);
    }
    evs
}

fn receipts(evs: &[AgentEvent]) -> Vec<ContextReceipt> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// G2: fallback windows for any provider (60% of ITS window)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_g2_request_when_fallback_is_any_providers_model_should_use_60_percent_of_its_window()
{
    let cases = [
        ("ollama:mistral", 19_660),
        ("openai:gpt-4", 4_915),
        ("ollama:codellama", 9_830),
        ("anthropic:claude-haiku-4-5", 120_000),
    ];
    for (fallback, limit) in cases {
        let (h, mock) = rig("anthropic:claude-opus-4-7", &[fallback]);
        // ~150K tokens of ~430-token messages: over every limit, under 600K.
        h.load_history(history(350, 100)).await;
        let (tx, _rx) = mpsc::channel(64);
        h.run_turn("next question", tx).await.expect("turn");
        let req = mock.last_request.lock().unwrap().clone().expect("request");
        let sent = total_tokens(&req.messages);
        assert!(sent <= limit, "fallback {fallback}: sent {sent} > {limit}");
        assert!(
            sent + 2_000 > limit,
            "fallback {fallback}: over-trimmed, sent {sent} of {limit}"
        );
    }
}

// ---------------------------------------------------------------------------
// G3: receipts across two consecutive fitted calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_g3_receipts_when_two_fitted_calls_should_record_fit_request_each_and_no_drop_oldest()
{
    let (h, _mock) = rig("anthropic:claude-opus-4-7", &["anthropic:claude-haiku-4-5"]);
    h.load_history(history(40, 1500)).await; // ~260K tokens, fitted to 120K
    let stored_before = h.get_messages().await.len();

    let r1 = receipts(&turn(&h).await);
    let r2 = receipts(&turn(&h).await);
    assert_eq!((r1.len(), r2.len()), (1, 1));

    for (i, r) in [&r1[0], &r2[0]].into_iter().enumerate() {
        let names: Vec<&str> = r
            .compaction
            .iter()
            .map(|s| s.strategy.name.as_str())
            .collect();
        assert_eq!(
            names.iter().filter(|n| **n == "fit_request").count(),
            1,
            "receipt {}: {names:?}",
            i + 1
        );
        assert!(
            !names.contains(&"drop_oldest"),
            "receipt {}: {names:?}",
            i + 1
        );
        let fit = r
            .compaction
            .iter()
            .find(|s| s.strategy.name == "fit_request")
            .unwrap();
        assert!(!fit.replaced.is_empty(), "left-out messages are listed");
        assert!(
            fit.replaced.len() as u32 + r.history_len >= stored_before as u32,
            "left out {} + sent {} < stored {}",
            fit.replaced.len(),
            r.history_len,
            stored_before
        );
    }
}

// ---------------------------------------------------------------------------
// G5: "context used" is the request sent, not the larger stored history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_g5_context_tokens_when_switched_to_smaller_model_should_report_the_request() {
    let (h, _mock) = rig("anthropic:claude-opus-4-7", &[]);
    h.load_history(history(62, 1500)).await; // ~400K stored
    h.set_model(ModelId::new("anthropic:claude-haiku-5-5"))
        .await;
    let _ = turn(&h).await;

    let used = h.context_tokens().await;
    assert!(used > 0, "gauge must show something");
    assert!(
        used <= 60_000,
        "gauge shows {used} tokens (> the 60K request); stored history is larger"
    );
}
