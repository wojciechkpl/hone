#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R1-R6 and the agent half of R10 of `docs/plans/stream-retry-fallback-spec.md`: the agent
//! retries a stream that fails before any output, like a failed request.
//!
//! A scripted provider returns a different result per call (`Ok(events)` or `Err`), counts its
//! calls, and can be told it runs its own tools. Calls past the script repeat its last entry.
//! The agent gets a millisecond `RetryConfig` (3 attempts). Every wait has a timeout.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_core::{AgentConfig, AgentError, AgentHandle, AgentMode, RetryConfig, TurnResult};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    TokenUsage, ToolApprovalPolicy, ToolCallRequest,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use serde_json::json;
use tokio::sync::{Notify, mpsc};

// -- scripted provider -----------------------------------------------------------------------------

#[derive(Clone)]
enum Call {
    Events(Vec<ProviderEvent>),
    Http(u16, &'static str),
    /// `Token`, then wait for the gate, then `Done` (R4).
    TokenThenWait,
}

struct Script {
    script: Vec<Call>,
    calls: Arc<AtomicUsize>,
    own_tools: bool,
    gate: Arc<Notify>,
    requests: Mutex<Vec<usize>>, // message counts of each request
}

impl Script {
    fn new(script: Vec<Call>) -> Arc<Self> {
        Self::build(script, false)
    }
    fn build(script: Vec<Call>, own_tools: bool) -> Arc<Self> {
        Arc::new(Self {
            script,
            calls: Arc::new(AtomicUsize::new(0)),
            own_tools,
            gate: Arc::new(Notify::new()),
            requests: Mutex::new(Vec::new()),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelProvider for Script {
    fn runs_own_tools(&self) -> bool {
        self.own_tools
    }
    fn id(&self) -> &str {
        "script"
    }
    fn display_name(&self) -> &str {
        "Script"
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
        r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        self.requests.lock().unwrap().push(r.messages.len());
        let n = self.calls.fetch_add(1, Ordering::SeqCst).min(self.script.len() - 1);
        match self.script[n].clone() {
            Call::Events(e) => Ok(Box::pin(stream::iter(e))),
            Call::Http(status, body) => Err(ProviderError::Http {
                provider: "script".to_owned(),
                status,
                body: body.to_owned(),
            }),
            Call::TokenThenWait => {
                let gate = Arc::clone(&self.gate);
                Ok(Box::pin(
                    stream::once(async { ProviderEvent::Token { text: "early".to_owned() } }).chain(
                        stream::once(async move {
                            gate.notified().await;
                            ProviderEvent::Done
                        }),
                    ),
                ))
            }
        }
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn tok(t: &str) -> ProviderEvent {
    ProviderEvent::Token { text: t.to_owned() }
}
fn err(m: &str) -> ProviderEvent {
    ProviderEvent::Error { message: m.to_owned() }
}
fn usage(input: u32, output: u32) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input,
        output,
        cache_read: 0,
        cache_creation: 0,
    })
}
fn events(e: Vec<ProviderEvent>) -> Call {
    Call::Events(e)
}
fn answer(text: &str) -> Call {
    events(vec![tok(text), ProviderEvent::Done])
}

fn handle(provider: Arc<Script>) -> AgentHandle {
    AgentHandle::spawn(AgentConfig {
        provider,
        tools: ToolRegistry::builder().build(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        // $1 per input token, free output: session cost == input tokens.
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 1_000_000.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
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
    })
}

async fn run(h: &AgentHandle) -> (Result<TurnResult, AgentError>, Vec<AgentEvent>) {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let res = tokio::time::timeout(Duration::from_secs(20), h.run_turn("go", tx))
        .await
        .expect("turn hung");
    let mut evs = Vec::new();
    while let Ok(e) = rx.try_recv() {
        evs.push(e);
    }
    (res, evs)
}

fn agent_errors(evs: &[AgentEvent]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn agent_tokens(evs: &[AgentEvent]) -> String {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// -- R1: a failure before output is retried ----------------------------------------------------------------

#[tokio::test]
async fn test_r1_error_before_output_is_retried_and_the_turn_succeeds() {
    let p = Script::new(vec![events(vec![err("Overloaded")]), answer("hello")]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.expect("the retry succeeds").response, "hello");
    assert_eq!(p.calls(), 2);
    assert!(agent_errors(&evs).is_empty(), "nothing of the failed attempt reaches the caller: {evs:?}");
    assert_eq!(agent_tokens(&evs), "hello");
}

#[tokio::test]
async fn test_r1_usage_then_error_before_output_is_retried_too() {
    let p = Script::new(vec![events(vec![usage(10, 0), err("Overloaded")]), answer("hello")]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.unwrap().response, "hello");
    assert_eq!(p.calls(), 2);
    assert!(agent_errors(&evs).is_empty(), "{evs:?}");
}

#[tokio::test]
async fn test_r1_request_errors_and_stream_failures_share_one_attempt_budget() {
    // Request error, stream failure, then success: 3 attempts.
    let p = Script::new(vec![Call::Http(503, "down"), events(vec![err("Overloaded")]), answer("hello")]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert_eq!(res.expect("third attempt succeeds").response, "hello");
    assert_eq!(p.calls(), 3);
}

#[tokio::test]
async fn test_r1_the_same_request_is_sent_on_every_attempt() {
    let p = Script::new(vec![events(vec![err("x")]), events(vec![err("x")]), answer("ok")]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    res.unwrap();
    let sizes = p.requests.lock().unwrap().clone();
    assert_eq!(sizes.len(), 3);
    assert!(sizes.iter().all(|n| *n == sizes[0]), "history must not change between attempts: {sizes:?}");
}

// -- R2: all attempts fail --------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r2_every_attempt_failing_before_output_fails_the_turn_with_the_providers_message() {
    let p = Script::new(vec![events(vec![err("Overloaded")])]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert!(matches!(res, Err(AgentError::Provider(_))), "{res:?}");
    assert_eq!(p.calls(), 3, "max_attempts attempts");
    assert_eq!(agent_errors(&evs), vec!["Overloaded".to_owned()], "exactly one Error, message unchanged");
}

// -- R3: failure after output is not retried -----------------------------------------------------------------------

#[tokio::test]
async fn test_r3_error_after_a_token_is_not_retried_and_the_token_was_delivered() {
    let p = Script::new(vec![events(vec![tok("partial"), err("cut")]), answer("never")]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert!(res.is_err());
    assert_eq!(p.calls(), 1);
    assert_eq!(agent_tokens(&evs), "partial");
    assert_eq!(agent_errors(&evs), vec!["cut".to_owned()]);
}

#[tokio::test]
async fn test_r3_error_after_a_tool_call_is_not_retried() {
    let call = ProviderEvent::ToolCall(ToolCallRequest {
        id: "c1".to_owned(),
        name: "read_file".to_owned(),
        arguments: json!({}),
    });
    let p = Script::new(vec![events(vec![call, err("cut")]), answer("never")]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert!(res.is_err());
    assert_eq!(p.calls(), 1);
}

#[tokio::test]
async fn test_r3_done_counts_as_output() {
    let p = Script::new(vec![events(vec![ProviderEvent::Done])]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert!(res.is_ok(), "an empty answer is an answer: {res:?}");
    assert_eq!(p.calls(), 1);
}

#[tokio::test]
async fn test_r3_a_stream_with_no_events_at_all_is_passed_on_not_retried() {
    let p = Script::new(vec![events(vec![]), answer("never")]);
    let h = handle(Arc::clone(&p));

    let (_res, evs) = run(&h).await;

    assert_eq!(p.calls(), 1, "an empty stream is not a failure before output");
    assert!(agent_errors(&evs).is_empty(), "{evs:?}");
}

// -- R4: streaming is preserved ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r4_events_reach_the_caller_as_they_arrive_once_output_has_started() {
    let p = Script::new(vec![Call::TokenThenWait]);
    let gate = Arc::clone(&p.gate);
    let h = handle(Arc::clone(&p));

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let h2 = h.clone();
    let turn = tokio::spawn(async move { h2.run_turn("go", tx).await });

    // The stream is still waiting for the gate: the first token must already be here.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::Token { .. }) {
                return;
            }
        }
        panic!("stream ended before a token");
    })
    .await
    .expect("the token was held back until the stream finished");

    gate.notify_one();
    let res = tokio::time::timeout(Duration::from_secs(10), turn).await.expect("turn hung").unwrap();
    assert!(res.is_ok(), "{res:?}");
    assert_eq!(p.calls(), 1);
}

// -- R5: usage from failed attempts is counted ------------------------------------------------------------------------------

#[tokio::test]
async fn test_r5_usage_reported_before_a_failure_is_counted_in_the_turn_and_the_session() {
    let p = Script::new(vec![
        events(vec![usage(10, 0), err("Overloaded")]),
        events(vec![usage(20, 5), tok("ok"), ProviderEvent::Done]),
    ]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    let turn = res.expect("second attempt succeeds");
    assert_eq!((turn.usage.input, turn.usage.output), (30, 5), "turn usage includes the failed attempt");
    let (session, cost) = h.session_spend().await;
    assert_eq!(session.input, 30);
    assert!((cost - 30.0).abs() < 1e-6, "session cost includes the failed attempt: {cost}");
}

// -- R6: providers that run their own tools -----------------------------------------------------------------------------------

#[tokio::test]
async fn test_r6_a_provider_running_its_own_tools_is_not_retried_after_its_stream_started() {
    let p = Script::build(vec![events(vec![err("cli-failed")]), answer("never")], true);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert!(res.is_err());
    assert_eq!(p.calls(), 1);
    assert_eq!(agent_errors(&evs), vec!["cli-failed".to_owned()]);
}

#[tokio::test]
async fn test_r6_request_errors_are_still_retried_for_such_a_provider() {
    let p = Script::build(vec![Call::Http(503, "down"), answer("ok")], true);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert_eq!(res.unwrap().response, "ok");
    assert_eq!(p.calls(), 2);
}

// -- R10: request-error classification ----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r10_http_500_504_529_request_errors_are_retried() {
    for (status, body) in [(500, "oops"), (504, "gateway timeout"), (529, "Overloaded")] {
        let p = Script::new(vec![Call::Http(status, body), answer("ok")]);
        let h = handle(Arc::clone(&p));

        let (res, _) = run(&h).await;

        assert_eq!(res.unwrap_or_else(|e| panic!("{status}: {e}")).response, "ok", "{status}");
        assert_eq!(p.calls(), 2, "HTTP {status}");
    }
}

#[tokio::test]
async fn test_r10_non_transient_request_error_is_not_retried() {
    let p = Script::new(vec![Call::Http(400, "bad request"), answer("never")]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert!(res.is_err());
    assert_eq!(p.calls(), 1);
}

#[tokio::test]
async fn test_r10_the_existing_text_check_still_retries_a_status_zero_error_mentioning_503() {
    // Nothing retried today may stop being retried: status 0 is decided by the text check.
    let p = Script::new(vec![Call::Http(0, "connection reset: 503 from proxy"), answer("ok")]);
    let h = handle(Arc::clone(&p));

    let (res, _) = run(&h).await;

    assert_eq!(res.unwrap().response, "ok");
    assert_eq!(p.calls(), 2);
}
