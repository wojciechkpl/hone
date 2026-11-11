#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]
//! N5 of `docs/plans/fallback-notice-spec.md`: the agent forwards each
//! `ProviderEvent::ModelFallback` as `AgentEvent::ModelFallback`, at its place in the stream,
//! adds no text, and the turn continues. The #85 retry rules are unchanged: the event counts
//! as output, so a call is never retried after it and the notice is not sent twice.
//!
//! Harness copied from `stream_retry_agent.rs` (scripted provider, millisecond retry config).

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_core::{AgentConfig, AgentError, AgentHandle, AgentMode, RetryConfig, TurnResult};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StreamErrorKind, TokenUsage, ToolApprovalPolicy, ToolCallRequest,
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
    /// `complete()` first signals `entered`, then waits for the gate (R13), then yields these events.
    GateThenEvents(Vec<ProviderEvent>),
    /// Same, then fails the request with this HTTP status.
    GateThenHttp(u16, &'static str),
}

struct Script {
    script: Vec<Call>,
    calls: Arc<AtomicUsize>,
    own_tools: bool,
    gate: Arc<Notify>,
    entered: Arc<Notify>,
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
            entered: Arc::new(Notify::new()),
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
        let n = self
            .calls
            .fetch_add(1, Ordering::SeqCst)
            .min(self.script.len() - 1);
        match self.script[n].clone() {
            Call::Events(e) => Ok(Box::pin(stream::iter(e))),
            Call::Http(status, body) => Err(ProviderError::Http {
                provider: "script".to_owned(),
                status,
                body: body.to_owned(),
            }),
            Call::GateThenEvents(e) => {
                self.entered.notify_one();
                self.gate.notified().await;
                Ok(Box::pin(stream::iter(e)))
            }
            Call::GateThenHttp(status, body) => {
                self.entered.notify_one();
                self.gate.notified().await;
                Err(ProviderError::Http {
                    provider: "script".to_owned(),
                    status,
                    body: body.to_owned(),
                })
            }
            Call::TokenThenWait => {
                let gate = Arc::clone(&self.gate);
                Ok(Box::pin(
                    stream::once(async {
                        ProviderEvent::Token {
                            text: "early".to_owned(),
                        }
                    })
                    .chain(stream::once(async move {
                        gate.notified().await;
                        ProviderEvent::Done
                    })),
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
    ProviderEvent::Error {
        message: m.to_owned(),
        kind: hone_proto::StreamErrorKind::Unclassified,
    }
}
fn err_kind(m: &str, kind: StreamErrorKind) -> ProviderEvent {
    ProviderEvent::Error {
        message: m.to_owned(),
        kind,
    }
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

fn mf(requested: &str, fallback: &str, reason: &str) -> ProviderEvent {
    ProviderEvent::ModelFallback {
        requested: requested.to_owned(),
        fallback: fallback.to_owned(),
        reason: reason.to_owned(),
    }
}

fn amf(requested: &str, fallback: &str, reason: &str) -> AgentEvent {
    AgentEvent::ModelFallback {
        requested: requested.to_owned(),
        fallback: fallback.to_owned(),
        reason: reason.to_owned(),
    }
}

fn notices(evs: &[AgentEvent]) -> Vec<AgentEvent> {
    evs.iter()
        .filter(|e| matches!(e, AgentEvent::ModelFallback { .. }))
        .cloned()
        .collect()
}

fn position(evs: &[AgentEvent], pred: impl Fn(&AgentEvent) -> bool) -> usize {
    evs.iter()
        .position(pred)
        .unwrap_or_else(|| panic!("event not found in {evs:?}"))
}

// -- N5 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_n5_the_notice_is_forwarded_with_the_same_fields_before_the_tokens() {
    let p = Script::new(vec![events(vec![
        mf("anthropic:claude-x", "openai:gpt-4o", "overloaded"),
        tok("hello"),
        ProviderEvent::Done,
    ])]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.expect("the turn succeeds").response, "hello");
    assert_eq!(
        notices(&evs),
        vec![amf("anthropic:claude-x", "openai:gpt-4o", "overloaded")]
    );
    let n = position(&evs, |e| matches!(e, AgentEvent::ModelFallback { .. }));
    let t = position(&evs, |e| matches!(e, AgentEvent::Token { .. }));
    assert!(n < t, "the notice comes before the call's tokens: {evs:?}");
    assert_eq!(p.calls(), 1);
}

#[tokio::test]
async fn test_n5_the_notice_adds_no_text_to_the_response_or_the_conversation() {
    let p = Script::new(vec![events(vec![
        mf("a:x", "b:y", "SECRET-REASON"),
        tok("hello"),
        ProviderEvent::Done,
    ])]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.unwrap().response, "hello");
    assert_eq!(agent_tokens(&evs), "hello", "no Token carries the notice");
    let msgs = h.get_messages().await;
    for m in &msgs {
        let text = m.content.as_text();
        assert!(
            !text.contains("SECRET-REASON") && !text.contains("b:y"),
            "the conversation must not contain the notice: {msgs:?}"
        );
    }
}

#[tokio::test]
async fn test_n5_usage_after_the_notice_still_counts_and_the_turn_completes() {
    let p = Script::new(vec![events(vec![
        mf("a:x", "b:y", "r"),
        usage(10, 0),
        tok("ok"),
        ProviderEvent::Done,
    ])]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    let turn = res.unwrap();
    assert_eq!(turn.response, "ok");
    assert!(
        evs.iter().any(|e| matches!(e, AgentEvent::Done { .. })),
        "{evs:?}"
    );
    assert_eq!(notices(&evs).len(), 1);
}

#[tokio::test]
async fn test_n5_a_failure_after_the_notice_is_not_retried_and_the_notice_is_sent_once() {
    // [ModelFallback, Error]: the notice is output, so the call is not retried (R3 of #85).
    let p = Script::new(vec![
        events(vec![mf("a:x", "b:y", "r"), err("boom")]),
        answer("never"),
    ]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert!(matches!(res, Err(AgentError::Provider(_))), "{res:?}");
    assert_eq!(p.calls(), 1, "no retry after the notice");
    assert_eq!(notices(&evs), vec![amf("a:x", "b:y", "r")]);
    assert_eq!(agent_errors(&evs), vec!["boom".to_owned()]);
}

#[tokio::test]
async fn test_n5_a_retried_call_without_a_notice_sends_none_and_a_later_notice_is_sent_once() {
    // Call 1 fails before output (retried, nothing forwarded); the retry's stream carries the notice.
    let p = Script::new(vec![
        events(vec![err("Overloaded")]),
        events(vec![
            mf("a:x", "b:y", "r"),
            tok("hello"),
            ProviderEvent::Done,
        ]),
    ]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.unwrap().response, "hello");
    assert_eq!(p.calls(), 2);
    assert_eq!(notices(&evs), vec![amf("a:x", "b:y", "r")]);
    assert!(agent_errors(&evs).is_empty(), "{evs:?}");
}

#[tokio::test]
async fn test_n5_each_model_call_of_a_turn_forwards_its_own_notice_in_place() {
    // Call 1: notice, then a tool call (unknown tool -> error result, turn goes on);
    // call 2: notice again, then the answer.
    let call = ProviderEvent::ToolCall(ToolCallRequest {
        id: "c1".to_owned(),
        name: "read_file".to_owned(),
        arguments: json!({}),
    });
    let p = Script::new(vec![
        events(vec![mf("a:x", "b:y", "r1"), call, ProviderEvent::Done]),
        events(vec![
            mf("a:x", "b:y", "r2"),
            tok("final"),
            ProviderEvent::Done,
        ]),
    ]);
    let h = handle(Arc::clone(&p));

    let (res, evs) = run(&h).await;

    assert_eq!(res.expect("turn completes").response, "final");
    assert_eq!(p.calls(), 2);
    assert_eq!(
        notices(&evs),
        vec![amf("a:x", "b:y", "r1"), amf("a:x", "b:y", "r2")],
        "the agent forwards every event; deduplication is the surfaces' job"
    );
    let first = position(&evs, |e| matches!(e, AgentEvent::ModelFallback { .. }));
    let tool = position(&evs, |e| matches!(e, AgentEvent::ToolCallStart { .. }));
    assert!(first < tool, "{evs:?}");
    let last_notice = evs
        .iter()
        .rposition(|e| matches!(e, AgentEvent::ModelFallback { .. }))
        .unwrap();
    let final_tok = position(
        &evs,
        |e| matches!(e, AgentEvent::Token { text } if text == "final"),
    );
    assert!(last_notice < final_tok, "{evs:?}");
}
