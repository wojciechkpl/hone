#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier, review side (spec H7): a review's cost is the reviewer turn's own
//! `cost_usd` (priced per call by the reviewer agent), not a re-pricing of its summed usage.
//! `ReviewResult` gains `cost_usd: f64`.
//!
//! The reviewer agent is created inside `ReviewerAgent::review`, so its prices come from the
//! process-wide default lookup. This binary installs the built-in table as that default once
//! (every test calls `init`), which is why it has its own file.
//!
//! The reviewer turn makes two calls (a tool call it has no tool for, then the answer), each
//! with a 60,000-token prompt: two base-rate requests although they add up to 120,000.
//! Expected: 2 x 60,000 x 0.10 + 2 x 100 x 0.50 = 12,100 per million = $0.0121 (the old
//! re-pricing of the summed usage would give the long rate: $0.0605).

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::ralph_loop::{RalphConfig, RalphLoop};
use hone_core::{
    AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig, ReviewerAgent,
    set_default_price_lookup,
};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    TokenUsage, ToolApprovalPolicy, ToolCallRequest,
};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider, PricingTable, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

const HAIKU: &str = "anthropic:claude-haiku-5-5";

static INIT: Once = Once::new();
fn init() {
    INIT.call_once(|| {
        let table = PricingTable::default_table();
        let lookup: PriceLookup = Arc::new(move |m: &ModelId| table.get(m).cloned());
        set_default_price_lookup(lookup);
    });
}

/// Two calls: a tool call (unknown to the reviewer), then an empty issue list.
struct ReviewerScript {
    next: AtomicUsize,
}

#[async_trait]
impl ModelProvider for ReviewerScript {
    fn id(&self) -> &str {
        "reviewer-script"
    }
    fn display_name(&self) -> &str {
        "Reviewer script"
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
        let use_ = |input, output| {
            ProviderEvent::Usage(TokenUsage {
                input,
                output,
                cache_read: 0,
                cache_creation: 0,
            })
        };
        let events = if self.next.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                use_(60_000, 0),
                use_(0, 100),
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "c1".to_owned(),
                    name: "no_such_tool".to_owned(),
                    arguments: serde_json::json!({}),
                }),
                ProviderEvent::Done,
            ]
        } else {
            vec![
                use_(60_000, 0),
                use_(0, 100),
                ProviderEvent::Token {
                    text: "[]".to_owned(),
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

fn config(provider: Arc<dyn ModelProvider>, model: &str) -> AgentConfig {
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
            long_prompt: None,
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
    }
}

fn reviewer_config() -> AgentConfig {
    config(
        Arc::new(ReviewerScript {
            next: AtomicUsize::new(0),
        }),
        HAIKU,
    )
}

#[tokio::test]
async fn test_h7_a_review_result_should_carry_the_reviewer_turns_own_cost() {
    init();
    let mut reviewer = ReviewerAgent::new(reviewer_config());
    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);

    let review = reviewer.review("fn main() {}", tx).await.expect("review");

    assert_eq!(review.usage.input, 120_000, "both calls are counted");
    assert!(
        (review.cost_usd - 0.0121).abs() < 1e-12,
        "two base-rate calls, not one 120K long request: {}",
        review.cost_usd
    );
}

#[tokio::test]
async fn test_h7_the_ralph_loop_should_charge_the_reviewer_turns_own_cost() {
    init();
    // The coder's model has no price (cost 0), so the total is the review's cost alone.
    let coder = AgentHandle::spawn(config(
        Arc::new(MockProvider::new(vec!["code".to_owned()])),
        "mock:coder",
    ));
    let ralph = RalphLoop::new(RalphConfig {
        max_iterations: 1,
        ..RalphConfig::default()
    });
    let (tx, _rx) = mpsc::channel::<AgentEvent>(256);

    let result = ralph
        .run(&coder, "write something", reviewer_config(), tx)
        .await
        .expect("ralph run");

    assert!(result.approved);
    assert!(
        (result.total_cost_usd - 0.0121).abs() < 1e-12,
        "review priced per call (base), not re-priced from the 120K sum: {}",
        result.total_cost_usd
    );
}
