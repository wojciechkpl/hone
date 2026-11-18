#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier through a real agent (spec H5/H6): the tier is chosen per provider
//! call by that call's prompt length (the running sum of input + cache read + cache write over
//! the call's usage events so far), so the output-only second event of the Anthropic adapter's
//! two-event pattern pays the long output rate when the prompt was long.
//!
//! The built-in `PricingTable` is the price lookup, installed on the agent with
//! `AgentHandle::set_price_lookup` (no process-wide state here). Worked numbers are the
//! spec's H6; model `anthropic:claude-haiku-5-5`.
//!
//! The scripted provider serves one entry of the script per `complete()` call, so a failed
//! attempt (usage then error, before any output) and its retry are two script entries.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, AgentHandle, AgentMode, PriceLookup, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StreamErrorKind, TokenUsage, ToolApprovalPolicy, ToolCallRequest,
};
use hone_providers::{CompletionRequest, ModelProvider, PricingTable, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

const HAIKU: &str = "anthropic:claude-haiku-5-5";

struct Script {
    calls: Vec<Vec<ProviderEvent>>,
    next: AtomicUsize,
}

impl Script {
    fn new(calls: Vec<Vec<ProviderEvent>>) -> Arc<Self> {
        Arc::new(Self {
            calls,
            next: AtomicUsize::new(0),
        })
    }
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
        let n = self
            .next
            .fetch_add(1, Ordering::SeqCst)
            .min(self.calls.len() - 1);
        Ok(Box::pin(stream::iter(self.calls[n].clone())))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn usage(input: u32, output: u32, cache_read: u32, cache_creation: u32) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input,
        output,
        cache_read,
        cache_creation,
    })
}
fn text(t: &str) -> ProviderEvent {
    ProviderEvent::Token { text: t.to_owned() }
}
fn tool_call() -> ProviderEvent {
    ProviderEvent::ToolCall(ToolCallRequest {
        id: "c1".to_owned(),
        name: "no_such_tool".to_owned(),
        arguments: serde_json::json!({}),
    })
}
fn fallback(to: &str) -> ProviderEvent {
    ProviderEvent::ModelFallback {
        requested: HAIKU.to_owned(),
        fallback: to.to_owned(),
        reason: "test".to_owned(),
    }
}
fn failed_attempt(first: ProviderEvent) -> Vec<ProviderEvent> {
    vec![
        first,
        ProviderEvent::Error {
            message: "Overloaded".to_owned(),
            kind: StreamErrorKind::Unclassified,
        },
    ]
}

fn builtin_lookup() -> PriceLookup {
    let table = PricingTable::default_table();
    Arc::new(move |m: &ModelId| table.get(m).cloned())
}

async fn spawn(script: Arc<Script>, model: &str) -> AgentHandle {
    let h = AgentHandle::spawn(AgentConfig {
        provider: script,
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new(model),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        // A placeholder that must NOT price the calls.
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
        retry_config: RetryConfig {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
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
    h.set_price_lookup(builtin_lookup()).await;
    h
}

/// Run one turn; returns (turn cost, session cost).
async fn cost_of(script: Arc<Script>) -> (f64, f64) {
    let h = spawn(script, HAIKU).await;
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let turn = tokio::time::timeout(Duration::from_secs(20), h.run_turn("go", tx))
        .await
        .expect("turn hung")
        .expect("turn should succeed");
    while rx.try_recv().is_ok() {}
    let (_, session) = h.session_spend().await;
    (turn.cost_usd, session)
}

fn assert_cost(label: &str, got: (f64, f64), want: f64) {
    assert!(
        (got.0 - want).abs() < 1e-12,
        "{label}: turn cost {} != {want}",
        got.0
    );
    assert!(
        (got.1 - want).abs() < 1e-12,
        "{label}: session cost {} != {want}",
        got.1
    );
}

#[tokio::test]
async fn test_h6_a_long_prompt_call_should_pay_the_long_rate_for_its_output_event_too() {
    // message_start (input + cache read), then message_delta (output only): prompt 110,000.
    let got = cost_of(Script::new(vec![vec![
        usage(90_000, 0, 20_000, 0),
        usage(0, 1_000, 0, 0),
        text("ok"),
        ProviderEvent::Done,
    ]]))
    .await;
    assert_cost("110K prompt", got, 0.0485);
}

#[tokio::test]
async fn test_h6_a_prompt_of_exactly_100000_tokens_should_pay_the_base_rate() {
    let got = cost_of(Script::new(vec![vec![
        usage(80_000, 0, 20_000, 0),
        usage(0, 1_000, 0, 0),
        text("ok"),
        ProviderEvent::Done,
    ]]))
    .await;
    assert_cost("exactly 100K prompt", got, 0.0087);
}

#[tokio::test]
async fn test_h6_the_second_call_of_a_turn_should_not_inherit_the_first_calls_prompt() {
    // Call 1 (tool call): prompt 150,000 -> long. Call 2 (answer): prompt 40,000 -> base.
    let got = cost_of(Script::new(vec![
        vec![
            usage(150_000, 0, 0, 0),
            usage(0, 500, 0, 0),
            tool_call(),
            ProviderEvent::Done,
        ],
        vec![
            usage(40_000, 0, 0, 0),
            usage(0, 500, 0, 0),
            text("done"),
            ProviderEvent::Done,
        ],
    ]))
    .await;
    assert_cost("two calls in a turn", got, 0.0805);
}

#[tokio::test]
async fn test_h6_a_single_event_request_should_be_priced_by_its_own_prompt_length() {
    // OpenAI-style single event: 60,000 input + 50,000 cache write + 100 output.
    let got = cost_of(Script::new(vec![vec![
        usage(60_000, 100, 0, 50_000),
        text("ok"),
        ProviderEvent::Done,
    ]]))
    .await;
    assert_cost("single event", got, 0.0615);
}

#[tokio::test]
async fn test_h5_a_mid_stream_fallback_should_restart_the_running_prompt_sum() {
    // 80,000 before the switch, 30,000 after: the fallback's own request has prompt 30,000,
    // not 110,000. 8,000 + 3,000 + 500 per million. (Passes before the change too; it
    // guards the restart once tiers exist.)
    let got = cost_of(Script::new(vec![vec![
        usage(80_000, 0, 0, 0),
        fallback(HAIKU),
        usage(30_000, 0, 0, 0),
        usage(0, 1_000, 0, 0),
        ProviderEvent::Done,
    ]]))
    .await;
    assert_cost("fallback after a base-rate prefix", got, 0.0115);
}

#[tokio::test]
async fn test_h5_usage_before_a_fallback_should_keep_its_own_tier() {
    // 150,000 before the switch is a long request (75,000 per million); after the switch the
    // fallback's request is 10,000 (base): 1,000 + 500. A sum that kept running would price
    // the second event at the long rate.
    let got = cost_of(Script::new(vec![vec![
        usage(150_000, 0, 0, 0),
        fallback("anthropic:claude-haiku-5-5-20261001"),
        usage(10_000, 0, 0, 0),
        usage(0, 1_000, 0, 0),
        ProviderEvent::Done,
    ]]))
    .await;
    assert_cost("fallback after a long prefix", got, 0.0765);
}

#[tokio::test]
async fn test_h5_a_failed_attempt_should_be_priced_alone_and_its_retry_by_its_own_prompt() {
    // Attempt 1 fails before output with a 120,000 prompt (long: 60,000 per million).
    // Attempt 2 is a separate request: 30,000 input + 100 output, base (3,000 + 50).
    let got = cost_of(Script::new(vec![
        failed_attempt(usage(120_000, 0, 0, 0)),
        vec![
            usage(30_000, 0, 0, 0),
            usage(0, 100, 0, 0),
            text("ok"),
            ProviderEvent::Done,
        ],
    ]))
    .await;
    assert_cost("failed long attempt then short retry", got, 0.06305);
}

#[tokio::test]
async fn test_h5_failed_attempts_should_not_be_summed_into_one_prompt() {
    // Three attempts of 70,000 each (two fail): each is its own base-rate request, even
    // though the three add up to 210,000. 7,000 x 3 + 50 per million. (Passes before the
    // change too; it guards against summing across attempts once tiers exist.)
    let got = cost_of(Script::new(vec![
        failed_attempt(usage(70_000, 0, 0, 0)),
        failed_attempt(usage(70_000, 0, 0, 0)),
        vec![
            usage(70_000, 0, 0, 0),
            usage(0, 100, 0, 0),
            text("ok"),
            ProviderEvent::Done,
        ],
    ]))
    .await;
    assert_cost("three 70K attempts", got, 0.02105);
}

#[tokio::test]
async fn test_h5_a_long_failed_attempt_event_by_event_should_each_use_its_own_prompt() {
    // One failed attempt reports two input-only events of 60,000: event by event (H2), each is
    // a base-rate request. 6,000 + 6,000, then the retry: 30,000 + 100 output. (Passes before
    // the change too; guards against summing events of a failed attempt.)
    let got = cost_of(Script::new(vec![
        vec![
            usage(60_000, 0, 0, 0),
            usage(60_000, 0, 0, 0),
            ProviderEvent::Error {
                message: "Overloaded".to_owned(),
                kind: StreamErrorKind::Unclassified,
            },
        ],
        vec![
            usage(30_000, 0, 0, 0),
            usage(0, 100, 0, 0),
            text("ok"),
            ProviderEvent::Done,
        ],
    ]))
    .await;
    assert_cost("two events in a failed attempt", got, 0.01505);
}
