#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! N2, N3, N4 and N6 of `docs/plans/fallback-notice-spec.md`.
//!
//! `FallbackChain` puts `ProviderEvent::ModelFallback { requested: members[0].id(),
//! fallback: members[k].id(), reason }` first in the stream when member k > 0 takes over.
//! `ModelDispatch` names its members by full model id. `CachingProvider` does not store a
//! response that contains the event.
//!
//! Assumption: "member failed" streams use `StreamErrorKind::Unclassified` (retryable); the
//! `reason` is the stream's error message, the request error's `Display`, or the `Unavailable`
//! reason, sanitized as N3 says.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{
    Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, StreamErrorKind, TokenUsage,
};
use hone_providers::{
    CachingProvider, CompletionRequest, FallbackChain, ModelDispatch, ModelProvider, ProviderError,
};

// -- scripted provider ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Call {
    Events(Vec<ProviderEvent>),
    Http(u16, &'static str),
    Timeout,
}

struct Scripted {
    id: String,
    script: Vec<Call>,
    calls: AtomicUsize,
    health: ProviderHealth,
}

impl Scripted {
    fn new(id: &str, script: Vec<Call>) -> Arc<Self> {
        Self::with_health(id, script, ProviderHealth::Healthy)
    }
    fn with_health(id: &str, script: Vec<Call>, health: ProviderHealth) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_owned(),
            script,
            calls: AtomicUsize::new(0),
            health,
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.id
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let n = self
            .calls
            .fetch_add(1, Ordering::SeqCst)
            .min(self.script.len() - 1);
        match self.script[n].clone() {
            Call::Events(e) => Ok(Box::pin(stream::iter(e))),
            Call::Http(status, body) => Err(ProviderError::Http {
                provider: self.id.clone(),
                status,
                body: body.to_owned(),
            }),
            Call::Timeout => Err(ProviderError::Timeout {
                provider: self.id.clone(),
            }),
        }
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        1
    }
    async fn health(&self) -> ProviderHealth {
        self.health.clone()
    }
}

fn tok(t: &str) -> ProviderEvent {
    ProviderEvent::Token { text: t.to_owned() }
}
fn done() -> ProviderEvent {
    ProviderEvent::Done
}
fn err(m: &str) -> ProviderEvent {
    err_kind(m, StreamErrorKind::Unclassified)
}
fn err_kind(m: &str, kind: StreamErrorKind) -> ProviderEvent {
    ProviderEvent::Error {
        message: m.to_owned(),
        kind,
    }
}
fn usage(n: u32) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input: n,
        output: 0,
        cache_read: 0,
        cache_creation: 0,
    })
}
fn mf(requested: &str, fallback: &str, reason: &str) -> ProviderEvent {
    ProviderEvent::ModelFallback {
        requested: requested.to_owned(),
        fallback: fallback.to_owned(),
        reason: reason.to_owned(),
    }
}
fn ok(events: Vec<ProviderEvent>) -> Call {
    Call::Events(events)
}
fn answer(t: &str) -> Call {
    ok(vec![tok(t), done()])
}
fn request() -> CompletionRequest {
    CompletionRequest::new(ModelId::new("mock:model"), vec![Message::user("hi")])
}
fn chain(members: Vec<Arc<Scripted>>) -> FallbackChain {
    FallbackChain::new(
        members
            .into_iter()
            .map(|m| m as Arc<dyn ModelProvider>)
            .collect(),
    )
}

async fn events_of(p: &dyn ModelProvider, r: CompletionRequest) -> Vec<ProviderEvent> {
    let s = tokio::time::timeout(Duration::from_secs(10), p.complete(r))
        .await
        .expect("complete hung")
        .expect("complete should return a stream");
    tokio::time::timeout(Duration::from_secs(10), s.collect::<Vec<_>>())
        .await
        .expect("stream hung")
}

fn count_fallbacks(events: &[ProviderEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ProviderEvent::ModelFallback { .. }))
        .count()
}

/// The reason of the single leading `ModelFallback` of a chain `a -> b` whose member `a`
/// failed with `failure` (a stream failure message).
async fn reason_for_stream_failure(failure: &str) -> String {
    let a = Scripted::new("a", vec![ok(vec![err(failure)])]);
    let b = Scripted::new("b", vec![answer("b")]);
    let ev = events_of(&chain(vec![a, b]), request()).await;
    match ev.first() {
        Some(ProviderEvent::ModelFallback { reason, .. }) => reason.clone(),
        other => panic!("expected a leading ModelFallback, got {other:?} in {ev:?}"),
    }
}

// -- N2 ------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_n2_first_member_answering_leaves_the_stream_unchanged() {
    let a = Scripted::new("a", vec![ok(vec![usage(3), tok("x"), tok("y"), done()])]);
    let b = Scripted::new("b", vec![answer("b")]);

    let ev = events_of(&chain(vec![a, Arc::clone(&b)]), request()).await;

    assert_eq!(ev, vec![usage(3), tok("x"), tok("y"), done()]);
    assert_eq!(b.calls(), 0);
}

#[tokio::test]
async fn test_n2_second_member_taking_over_puts_model_fallback_first() {
    let a = Scripted::new("a", vec![ok(vec![err("Overloaded")])]);
    let b = Scripted::new("b", vec![answer("hello")]);

    let ev = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(ev, vec![mf("a", "b", "Overloaded"), tok("hello"), done()]);
}

#[tokio::test]
async fn test_n2_carried_usage_follows_the_notice_and_precedes_the_answer() {
    let a = Scripted::new("a", vec![ok(vec![usage(10), err("Overloaded")])]);
    let b = Scripted::new("b", vec![answer("hi")]);

    let ev = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        ev,
        vec![mf("a", "b", "Overloaded"), usage(10), tok("hi"), done()]
    );
}

#[tokio::test]
async fn test_n2_several_failing_members_give_one_notice_naming_the_first_and_the_answerer() {
    let a = Scripted::new("a", vec![ok(vec![usage(1), err("a-failed")])]);
    let b = Scripted::new("b", vec![ok(vec![usage(2), err("b-failed")])]);
    let c = Scripted::new("c", vec![answer("c-answer")]);

    let ev = events_of(&chain(vec![a, b, c]), request()).await;

    assert_eq!(count_fallbacks(&ev), 1, "{ev:?}");
    assert_eq!(
        ev,
        vec![
            mf("a", "c", "a-failed"),
            usage(1),
            usage(2),
            tok("c-answer"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_n2_a_request_error_in_between_does_not_add_a_second_notice() {
    let a = Scripted::new("a", vec![ok(vec![err("a-failed")])]);
    let b = Scripted::new("b", vec![Call::Http(503, "b-down")]);
    let c = Scripted::new("c", vec![answer("c")]);

    let ev = events_of(&chain(vec![a, b, c]), request()).await;

    assert_eq!(ev, vec![mf("a", "c", "a-failed"), tok("c"), done()]);
}

#[tokio::test]
async fn test_n2_every_member_failing_in_the_stream_reports_as_before_without_a_notice() {
    let a = Scripted::new("a", vec![ok(vec![usage(10), err("a-failed")])]);
    let b = Scripted::new("b", vec![ok(vec![usage(20), err("b-failed")])]);

    let ev = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(count_fallbacks(&ev), 0, "{ev:?}");
    assert_eq!(ev, vec![usage(10), usage(20), err("b-failed")]);
}

#[tokio::test]
async fn test_n2_every_member_failing_the_request_returns_the_last_error_without_a_notice() {
    let a = Scripted::new("a", vec![Call::Http(503, "a-down")]);
    let b = Scripted::new("b", vec![Call::Http(503, "b-down")]);

    let r = chain(vec![a, b]).complete(request()).await;

    match r {
        Err(ProviderError::Http {
            status: 503, body, ..
        }) => assert_eq!(body, "b-down"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("expected an error, got a stream"),
    }
}

#[tokio::test]
async fn test_n2_a_non_retried_failure_of_the_taking_over_member_is_still_a_switch() {
    let a = Scripted::new("a", vec![ok(vec![usage(5), err("a-failed")])]);
    let b = Scripted::new(
        "b",
        vec![ok(vec![
            usage(6),
            err_kind(
                "bad request",
                StreamErrorKind::Provider { status: Some(400) },
            ),
        ])],
    );
    let c = Scripted::new("c", vec![answer("never")]);

    let ev = events_of(&chain(vec![a, b, Arc::clone(&c)]), request()).await;

    assert_eq!(
        ev,
        vec![
            mf("a", "b", "a-failed"),
            usage(5),
            usage(6),
            err_kind(
                "bad request",
                StreamErrorKind::Provider { status: Some(400) }
            ),
        ]
    );
    assert_eq!(c.calls(), 0);
}

#[tokio::test]
async fn test_n2_a_non_retried_failure_of_the_first_member_has_no_notice() {
    let a = Scripted::new(
        "a",
        vec![ok(vec![err_kind(
            "unauthorized",
            StreamErrorKind::Provider { status: Some(401) },
        )])],
    );
    let b = Scripted::new("b", vec![answer("never")]);

    let ev = events_of(&chain(vec![a, Arc::clone(&b)]), request()).await;

    assert_eq!(
        ev,
        vec![err_kind(
            "unauthorized",
            StreamErrorKind::Provider { status: Some(401) }
        )]
    );
    assert_eq!(b.calls(), 0);
}

#[tokio::test]
async fn test_n2_a_failure_after_output_has_no_notice() {
    let a = Scripted::new("a", vec![ok(vec![tok("partial"), err("cut")])]);
    let b = Scripted::new("b", vec![answer("b")]);

    let ev = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(ev, vec![tok("partial"), err("cut")]);
}

// -- N3: reasons ---------------------------------------------------------------------------------

#[tokio::test]
async fn test_n3_a_request_error_reason_is_its_display() {
    let expected_http = ProviderError::Http {
        provider: "a".into(),
        status: 503,
        body: "service down".into(),
    }
    .to_string();
    let expected_timeout = ProviderError::Timeout {
        provider: "a".into(),
    }
    .to_string();

    for (call, expected) in [
        (Call::Http(503, "service down"), expected_http),
        (Call::Timeout, expected_timeout),
    ] {
        let a = Scripted::new("a", vec![call]);
        let b = Scripted::new("b", vec![answer("b")]);

        let ev = events_of(&chain(vec![a, b]), request()).await;

        assert_eq!(ev, vec![mf("a", "b", &expected), tok("b"), done()]);
    }
}

#[tokio::test]
async fn test_n3_an_unavailable_member_reason_is_its_health_reason() {
    let a = Scripted::with_health(
        "a",
        vec![answer("never")],
        ProviderHealth::Unavailable {
            reason: "offline".to_owned(),
        },
    );
    let b = Scripted::new("b", vec![answer("b")]);

    let ev = events_of(&chain(vec![Arc::clone(&a), b]), request()).await;

    assert_eq!(ev, vec![mf("a", "b", "offline"), tok("b"), done()]);
    assert_eq!(a.calls(), 0);
}

#[tokio::test]
async fn test_n3_the_reason_comes_from_member_zero_not_from_later_failures() {
    let a = Scripted::new("a", vec![ok(vec![err("first reason")])]);
    let b = Scripted::new("b", vec![Call::Http(503, "second reason")]);
    let c = Scripted::new("c", vec![answer("c")]);

    let ev = events_of(&chain(vec![a, b, c]), request()).await;

    assert_eq!(ev[0], mf("a", "c", "first reason"));
}

#[tokio::test]
async fn test_n3_reason_sanitization_table() {
    let long_200 = "x".repeat(200);
    let long_201 = "x".repeat(201);
    let multibyte = "é".repeat(250);
    let emoji = "😀".repeat(201);
    let cases: Vec<(String, String)> = vec![
        ("plain text".into(), "plain text".into()),
        ("line1\nline2".into(), "line1 line2".into()),
        ("a\r\nb".into(), "a b".into()),
        ("tab\there".into(), "tab here".into()),
        (
            "red \u{1b}[31mtext\u{1b}[0m".into(),
            "red [31mtext [0m".into(),
        ),
        ("x\u{7f}y".into(), "x y".into()),
        ("x\u{85}y".into(), "x y".into()),
        ("  lots    of \n\n  space  ".into(), "lots of space".into()),
        ("\n\t ".into(), "unknown error".into()),
        ("".into(), "unknown error".into()),
        (long_200.clone(), long_200),
        (long_201, format!("{}…", "x".repeat(200))),
        (multibyte, format!("{}…", "é".repeat(200))),
        (emoji, format!("{}…", "😀".repeat(200))),
    ];

    for (input, expected) in cases {
        assert_eq!(
            reason_for_stream_failure(&input).await,
            expected,
            "input {input:?}"
        );
    }
}

#[tokio::test]
async fn test_n3_collapse_happens_before_the_length_cut() {
    // 150 chars, a run of newlines, 100 chars: collapsed = 251 chars -> cut at 200.
    let input = format!("{}\n\n\n\n\n{}", "a".repeat(150), "b".repeat(100));
    let expected = format!("{} {}…", "a".repeat(150), "b".repeat(49));

    assert_eq!(reason_for_stream_failure(&input).await, expected);
}

// -- N4: ModelDispatch ---------------------------------------------------------------------------

fn dispatch_with(
    openai: Arc<Scripted>,
    ollama: Arc<Scripted>,
    fallback: Vec<&str>,
) -> ModelDispatch {
    let d = ModelDispatch::new(ModelId::new("openai:gpt-x"))
        .with_provider("openai", openai as Arc<dyn ModelProvider>)
        .with_provider("ollama", ollama as Arc<dyn ModelProvider>);
    if fallback.is_empty() {
        d
    } else {
        d.with_fallback(fallback.into_iter().map(ModelId::new).collect())
    }
}

fn dreq(model: &str) -> CompletionRequest {
    CompletionRequest::new(ModelId::new(model), vec![Message::user("p")])
}

#[tokio::test]
async fn test_n4_dispatch_names_the_full_model_ids_in_the_notice() {
    let openai = Scripted::new("openai", vec![ok(vec![err("overloaded")])]);
    let ollama = Scripted::new("ollama", vec![answer("fb")]);
    let d = dispatch_with(openai, ollama, vec!["ollama:fb-model"]);

    let ev = events_of(&d, dreq("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "ollama:fb-model", "overloaded"),
            tok("fb"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_n4_dispatch_names_the_fallback_that_took_over_among_several() {
    let openai = Scripted::new("openai", vec![ok(vec![err("first")])]);
    let ollama = Scripted::new("ollama", vec![ok(vec![err("second")])]);
    let anthropic = Scripted::new("anthropic", vec![answer("third")]);
    let d = ModelDispatch::new(ModelId::new("openai:gpt-x"))
        .with_provider("openai", openai as Arc<dyn ModelProvider>)
        .with_provider("ollama", ollama as Arc<dyn ModelProvider>)
        .with_provider("anthropic", anthropic as Arc<dyn ModelProvider>)
        .with_fallback(vec![
            ModelId::new("ollama:fb1"),
            ModelId::new("anthropic:claude-fb"),
        ]);

    let ev = events_of(&d, dreq("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "anthropic:claude-fb", "first"),
            tok("third"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_n4_a_routed_request_names_its_own_model_as_requested() {
    let openai = Scripted::new("openai", vec![answer("openai-backup")]);
    let ollama = Scripted::new("ollama", vec![ok(vec![err("down")])]);
    let d = dispatch_with(openai, ollama, vec!["openai:backup"]);

    let ev = events_of(&d, dreq("ollama:routed")).await;

    assert_eq!(
        ev,
        vec![
            mf("ollama:routed", "openai:backup", "down"),
            tok("openai-backup"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_n4_dispatch_with_a_fallback_list_is_unchanged_when_the_requested_model_answers() {
    let openai = Scripted::new("openai", vec![answer("fine")]);
    let ollama = Scripted::new("ollama", vec![answer("fb")]);
    let d = dispatch_with(openai, Arc::clone(&ollama), vec!["ollama:fb"]);

    let ev = events_of(&d, dreq("openai:gpt-x")).await;

    assert_eq!(ev, vec![tok("fine"), done()]);
    assert_eq!(ollama.calls(), 0);
}

#[tokio::test]
async fn test_n4_dispatch_without_a_fallback_list_emits_no_notice_and_passes_failures_on() {
    let openai = Scripted::new("openai", vec![ok(vec![err("boom")])]);
    let ollama = Scripted::new("ollama", vec![answer("never")]);
    let d = dispatch_with(openai, ollama, vec![]);

    let ev = events_of(&d, dreq("openai:gpt-x")).await;

    assert_eq!(ev, vec![err("boom")]);
}

#[tokio::test]
async fn test_n4_dispatch_with_a_fallback_list_where_every_model_fails_has_no_notice() {
    let openai = Scripted::new("openai", vec![ok(vec![err("first")])]);
    let ollama = Scripted::new("ollama", vec![ok(vec![err("last")])]);
    let d = dispatch_with(openai, ollama, vec!["ollama:fb"]);

    let ev = events_of(&d, dreq("openai:gpt-x")).await;

    assert_eq!(ev, vec![err("last")]);
}

// -- N6: CachingProvider ---------------------------------------------------------------------------

#[tokio::test]
async fn test_n6_a_response_with_a_model_fallback_is_passed_on_but_not_stored() {
    let inner = Scripted::new(
        "inner",
        vec![ok(vec![mf("a:x", "b:y", "overloaded"), tok("fb"), done()])],
    );
    let cached = CachingProvider::new(Arc::clone(&inner) as Arc<dyn ModelProvider>, 16);

    let first = events_of(&cached, request()).await;
    let second = events_of(&cached, request()).await;

    assert_eq!(
        first,
        vec![mf("a:x", "b:y", "overloaded"), tok("fb"), done()]
    );
    assert_eq!(second, first);
    assert_eq!(
        inner.calls(),
        2,
        "the second identical request must reach the provider again"
    );
}

#[tokio::test]
async fn test_n6_a_later_plain_answer_replaces_the_uncached_fallback_one_and_is_stored() {
    let inner = Scripted::new(
        "inner",
        vec![
            ok(vec![mf("a:x", "b:y", "overloaded"), tok("fb"), done()]),
            answer("primary-back"),
        ],
    );
    let cached = CachingProvider::new(Arc::clone(&inner) as Arc<dyn ModelProvider>, 16);

    let _ = events_of(&cached, request()).await;
    let second = events_of(&cached, request()).await;
    let third = events_of(&cached, request()).await;

    assert_eq!(second, vec![tok("primary-back"), done()]);
    assert_eq!(third, second);
    assert_eq!(inner.calls(), 2, "the plain answer is cached");
}
