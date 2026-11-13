#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U3 and U4 of `docs/plans/usage-and-pricing-spec.md` at the agent: usage is priced with the
//! entry of the model that served the call, a model without an entry costs 0 and is reported.
//!
//! API (new), chosen so `AgentConfig` (~99 struct literals) stays untouched:
//! - `hone_core::PriceLookup = Arc<dyn Fn(&ModelId) -> Option<PricingEntry> + Send + Sync>`;
//! - `AgentHandle::set_price_lookup(&self, PriceLookup)` (fire-and-forget like `set_tools`):
//!   from then on every usage is priced with `lookup(model that served the call)`. `None` means
//!   no price: cost 0, usage still counted. Without a lookup, `AgentConfig.pricing` applies as
//!   before. The model that served a call is the session's model (`set_model`, routing) or the
//!   `fallback` of a `ProviderEvent::ModelFallback` seen in that call;
//! - `AgentHandle::unpriced_models(&self) -> Vec<String>`: the model ids that had at least one
//!   unpriced call this session, in first-seen order, without duplicates. The CLI prints the
//!   one-line note from it and sets `cost_known: false` when it is not empty.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig, TurnResult};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    TokenUsage, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

/// Every call reports 1M input tokens; `fallback_on` (call numbers, 0-based) first announce a
/// fallback to `mock:fallback`. Records the model each request asked for.
struct Script {
    n: AtomicUsize,
    fallback_on: Vec<usize>,
    asked: Mutex<Vec<String>>,
}

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
        r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        self.asked.lock().unwrap().push(r.model.as_str().to_owned());
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let mut events = Vec::new();
        if self.fallback_on.contains(&i) {
            events.push(ProviderEvent::ModelFallback {
                requested: r.model.as_str().to_owned(),
                fallback: "mock:fallback".to_owned(),
                reason: "test".to_owned(),
            });
        }
        events.push(ProviderEvent::Token { text: "ok".to_owned() });
        events.push(ProviderEvent::Usage(TokenUsage {
            input: 1_000_000,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        }));
        events.push(ProviderEvent::Done);
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn entry(model: &str, input_per_million: f64) -> PricingEntry {
    PricingEntry {
        model: ModelId::new(model),
        input_per_million,
        output_per_million: 0.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    }
}

/// mock:a $1 per 1M input tokens, mock:b $10, mock:fallback $100; anything else unpriced.
fn lookup() -> PriceLookup {
    Arc::new(|m: &ModelId| match m.as_str() {
        "mock:a" => Some(entry("mock:a", 1.0)),
        "mock:b" => Some(entry("mock:b", 10.0)),
        "mock:fallback" => Some(entry("mock:fallback", 100.0)),
        _ => None,
    })
}

async fn handle(fallback_on: Vec<usize>) -> AgentHandle {
    let provider = Arc::new(Script {
        n: AtomicUsize::new(0),
        fallback_on,
        asked: Mutex::new(Vec::new()),
    });
    let h = AgentHandle::spawn(AgentConfig {
        provider,
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:a"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        // A deliberately wrong single price ($3 per 1M input, the old hard-coded one):
        // the lookup must win over it.
        pricing: entry("mock:a", 3.0),
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

async fn turn(h: &AgentHandle) -> TurnResult {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let r = h.run_turn("hello", tx).await.expect("turn");
    while rx.try_recv().is_ok() {}
    r
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[tokio::test]
async fn test_u4_a_turn_should_be_priced_with_the_entry_of_the_sessions_model() {
    let h = handle(vec![]).await;

    let r = turn(&h).await;

    assert!(close(r.cost_usd, 1.0), "mock:a is $1 per 1M, not the config's $3: {}", r.cost_usd);
    assert_eq!(r.usage.input, 1_000_000);
}

#[tokio::test]
async fn test_u4_changing_the_model_should_price_the_next_turn_with_the_new_entry() {
    let h = handle(vec![]).await;

    let first = turn(&h).await;
    h.set_model(ModelId::new("mock:b")).await;
    let second = turn(&h).await;
    h.set_model(ModelId::new("mock:a")).await;
    let third = turn(&h).await;

    assert!(close(first.cost_usd, 1.0), "{}", first.cost_usd);
    assert!(close(second.cost_usd, 10.0), "routed/switched model: {}", second.cost_usd);
    assert!(close(third.cost_usd, 1.0), "{}", third.cost_usd);
}

#[tokio::test]
async fn test_u4_a_fallback_model_should_price_the_call_it_served() {
    let h = handle(vec![0]).await;

    let r = turn(&h).await;

    assert!(
        close(r.cost_usd, 100.0),
        "the call was served by mock:fallback ($100 per 1M): {}",
        r.cost_usd
    );
}

#[tokio::test]
async fn test_u3_a_model_without_an_entry_should_cost_nothing_but_keep_its_usage() {
    let h = handle(vec![]).await;
    h.set_model(ModelId::new("ollama:llama3")).await;

    let r = turn(&h).await;

    assert_eq!(r.cost_usd, 0.0);
    assert_eq!(r.usage.input, 1_000_000, "usage is still counted");
}

#[tokio::test]
async fn test_u3_unpriced_models_should_be_listed_once_in_first_seen_order() {
    let h = handle(vec![]).await;
    assert!(h.unpriced_models().await.is_empty(), "nothing unpriced yet");

    turn(&h).await; // mock:a, priced
    assert!(h.unpriced_models().await.is_empty(), "priced calls are not listed");
    h.set_model(ModelId::new("ollama:llama3")).await;
    turn(&h).await;
    turn(&h).await;
    h.set_model(ModelId::new("openai:unknown")).await;
    turn(&h).await;
    h.set_model(ModelId::new("ollama:llama3")).await;
    turn(&h).await;

    assert_eq!(h.unpriced_models().await, ["ollama:llama3", "openai:unknown"]);
}

#[tokio::test]
async fn test_u3_the_session_cost_should_count_only_priced_calls() {
    let h = handle(vec![]).await;

    let a = turn(&h).await;
    h.set_model(ModelId::new("ollama:llama3")).await;
    let free = turn(&h).await;
    h.set_model(ModelId::new("mock:b")).await;
    let b = turn(&h).await;

    let total = a.cost_usd + free.cost_usd + b.cost_usd;
    assert!(close(total, 11.0), "{total}");
}
