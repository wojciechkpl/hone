#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R7, R8, R9 of `docs/plans/stream-retry-fallback-spec.md`: `FallbackChain` and
//! `TierFallbackProvider` move on when a member fails before any output.
//!
//! A scripted provider returns a different result per call (`Ok(events)` or an `Err` built from
//! an `ErrSpec`), counts its calls, and has a configurable `runs_own_tools`. The calls past the
//! script repeat its last entry. No timing is involved.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{
    Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, StreamErrorKind, TokenUsage,
};
use hone_providers::{
    CompletionRequest, FallbackChain, ModelProvider, ModelRouter, ProviderError, ProviderRegistry,
    ProviderTier, RoutePolicy, TierFallbackProvider,
};
use tokio_util::sync::CancellationToken;

// -- scripted provider ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum ErrSpec {
    Http(u16, &'static str),
    Timeout,
    Stream,
}

#[derive(Clone, Debug)]
enum Call {
    Events(Vec<ProviderEvent>),
    Fail(ErrSpec),
}

struct Scripted {
    id: String,
    script: Vec<Call>,
    calls: Arc<AtomicUsize>,
    own_tools: bool,
    health: ProviderHealth,
}

impl Scripted {
    fn new(id: &str, script: Vec<Call>) -> (Arc<Self>, Arc<AtomicUsize>) {
        Self::build(id, script, false, ProviderHealth::Healthy)
    }
    fn own_tools(id: &str, script: Vec<Call>) -> (Arc<Self>, Arc<AtomicUsize>) {
        Self::build(id, script, true, ProviderHealth::Healthy)
    }
    fn build(
        id: &str,
        script: Vec<Call>,
        own_tools: bool,
        health: ProviderHealth,
    ) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                id: id.to_owned(),
                script,
                calls: Arc::clone(&calls),
                own_tools,
                health,
            }),
            calls,
        )
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn runs_own_tools(&self) -> bool {
        self.own_tools
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        "Scripted"
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
            Call::Fail(ErrSpec::Http(status, body)) => Err(ProviderError::Http {
                provider: self.id.clone(),
                status,
                body: body.to_owned(),
            }),
            Call::Fail(ErrSpec::Timeout) => Err(ProviderError::Timeout {
                provider: self.id.clone(),
            }),
            Call::Fail(ErrSpec::Stream) => Err(ProviderError::Stream {
                provider: self.id.clone(),
                source: std::io::Error::other("reset"),
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
fn usage(input: u32) -> ProviderEvent {
    ProviderEvent::Usage(TokenUsage {
        input,
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
fn request() -> CompletionRequest {
    CompletionRequest::new(ModelId::new("mock:model"), vec![Message::user("hi")])
}
fn count(c: &Arc<AtomicUsize>) -> usize {
    c.load(Ordering::SeqCst)
}

async fn events_of(p: &dyn ModelProvider, req: CompletionRequest) -> Vec<ProviderEvent> {
    let s = tokio::time::timeout(std::time::Duration::from_secs(10), p.complete(req))
        .await
        .expect("complete hung")
        .expect("complete should return a stream");
    tokio::time::timeout(std::time::Duration::from_secs(10), s.collect::<Vec<_>>())
        .await
        .expect("stream hung")
}

async fn expect_err(p: &dyn ModelProvider, req: CompletionRequest) -> ProviderError {
    match tokio::time::timeout(std::time::Duration::from_secs(10), p.complete(req))
        .await
        .expect("complete hung")
    {
        Err(e) => e,
        Ok(_) => panic!("expected a request error"),
    }
}

fn chain(members: Vec<Arc<Scripted>>) -> FallbackChain {
    FallbackChain::new(
        members
            .into_iter()
            .map(|m| m as Arc<dyn ModelProvider>)
            .collect(),
    )
}

// -- R7: cancellation is never retried ---------------------------------------------------------------

#[tokio::test]
async fn test_r7_cancelled_request_passes_the_stream_failure_on_without_trying_the_next_member() {
    let (a, a_calls) = Scripted::new("a", vec![ok(vec![err("cancelled")])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b-answer"), ProviderEvent::Done])]);
    let c = chain(vec![a, b]);
    let token = CancellationToken::new();
    token.cancel();
    let mut req = request();
    req.cancel = Some(token);

    let events = events_of(&c, req).await;

    assert_eq!(events, vec![err("cancelled")]);
    assert_eq!(count(&a_calls), 1);
    assert_eq!(
        count(&b_calls),
        0,
        "a cancelled request must not be retried elsewhere"
    );
}

// -- R8: FallbackChain ---------------------------------------------------------------------------------

#[tokio::test]
async fn test_r8_member_failing_before_output_hands_over_to_the_next_member() {
    let (a, a_calls) = Scripted::new("a", vec![ok(vec![err("Overloaded")])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("hello"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![
            mf("a", "b", "Overloaded"),
            tok("hello"),
            ProviderEvent::Done
        ]
    );
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 1));
}

#[tokio::test]
async fn test_r8_transient_request_errors_move_on() {
    for spec in [
        ErrSpec::Http(503, "down"),
        ErrSpec::Http(500, "oops"),
        ErrSpec::Http(529, "Overloaded"),
        ErrSpec::Timeout,
        ErrSpec::Stream,
    ] {
        let (a, a_calls) = Scripted::new("a", vec![Call::Fail(spec.clone())]);
        let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

        let events = events_of(&chain(vec![a, b]), request()).await;

        assert!(
            matches!(
                events.first(),
                Some(ProviderEvent::ModelFallback { requested, fallback, .. })
                    if requested == "a" && fallback == "b"
            ),
            "{spec:?}: {events:?}"
        );
        assert_eq!(events[1..], [tok("b"), ProviderEvent::Done], "{spec:?}");
        assert_eq!((count(&a_calls), count(&b_calls)), (1, 1), "{spec:?}");
    }
}

#[tokio::test]
async fn test_r8_non_transient_request_error_is_returned_at_once() {
    let (a, _) = Scripted::new("a", vec![Call::Fail(ErrSpec::Http(400, "bad request"))]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let e = expect_err(&chain(vec![a, b]), request()).await;

    assert!(
        matches!(e, ProviderError::Http { status: 400, .. }),
        "{e:?}"
    );
    assert_eq!(count(&b_calls), 0);
}

#[tokio::test]
async fn test_r8_usage_of_a_failed_member_is_passed_on_ahead_of_the_next_members_events() {
    let (a, _) = Scripted::new("a", vec![ok(vec![usage(10), err("Overloaded")])]);
    let (b, _) = Scripted::new("b", vec![ok(vec![tok("hi"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![
            mf("a", "b", "Overloaded"),
            usage(10),
            tok("hi"),
            ProviderEvent::Done
        ]
    );
}

#[tokio::test]
async fn test_r8_all_members_failing_before_output_report_the_last_members_failure_in_the_stream() {
    let (a, a_calls) = Scripted::new("a", vec![ok(vec![usage(10), err("a-failed")])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![usage(20), err("b-failed")])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(events, vec![usage(10), usage(20), err("b-failed")]);
    assert_eq!(
        (count(&a_calls), count(&b_calls)),
        (1, 1),
        "each member at most once"
    );
}

#[tokio::test]
async fn test_r8_last_member_failing_the_request_is_reported_as_that_error() {
    let (a, _) = Scripted::new("a", vec![ok(vec![err("a-failed")])]);
    let (b, _) = Scripted::new("b", vec![Call::Fail(ErrSpec::Http(503, "b-down"))]);

    let e = expect_err(&chain(vec![a, b]), request()).await;

    match e {
        ProviderError::Http {
            status: 503, body, ..
        } => assert_eq!(body, "b-down"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn test_r8_failure_after_output_is_passed_on_and_the_next_member_is_not_tried() {
    let (a, a_calls) = Scripted::new("a", vec![ok(vec![tok("partial"), err("cut")])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(events, vec![tok("partial"), err("cut")]);
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 0));
}

#[tokio::test]
async fn test_r8_a_member_that_runs_its_own_tools_is_not_retried_after_its_stream_started() {
    let (a, a_calls) = Scripted::own_tools("a", vec![ok(vec![err("cli-failed")])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(events, vec![err("cli-failed")]);
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 0));
}

#[tokio::test]
async fn test_r8_unavailable_member_is_still_skipped_without_being_called() {
    let (a, a_calls) = Scripted::build(
        "a",
        vec![ok(vec![tok("a"), ProviderEvent::Done])],
        false,
        ProviderHealth::Unavailable {
            reason: "offline".to_owned(),
        },
    );
    let (b, _) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![mf("a", "b", "offline"), tok("b"), ProviderEvent::Done]
    );
    assert_eq!(count(&a_calls), 0);
}

#[tokio::test]
async fn test_r8_healthy_first_member_streams_through_untouched() {
    let (a, a_calls) = Scripted::new("a", vec![ok(vec![tok("x"), tok("y"), ProviderEvent::Done])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(events, vec![tok("x"), tok("y"), ProviderEvent::Done]);
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 0));
}

// -- R9: TierFallbackProvider ----------------------------------------------------------------------------

/// Primary plus tier candidates `c1`, `c2` (cheapest first). Only the candidates are registered.
struct Tier {
    fallback: TierFallbackProvider,
    primary_calls: Arc<AtomicUsize>,
    c1_calls: Arc<AtomicUsize>,
    c2_calls: Arc<AtomicUsize>,
}

fn tier(
    primary: (Arc<Scripted>, Arc<AtomicUsize>),
    c1: (Arc<Scripted>, Arc<AtomicUsize>),
    c2: (Arc<Scripted>, Arc<AtomicUsize>),
) -> Tier {
    let registry = Arc::new(ProviderRegistry::new());
    registry.register(c1.0 as Arc<dyn ModelProvider>);
    registry.register(c2.0 as Arc<dyn ModelProvider>);
    let tiers = ["c1", "c2"]
        .iter()
        .enumerate()
        .map(|(i, id)| ProviderTier {
            provider_id: (*id).to_owned(),
            model: None,
            cost_per_1k_input_usd: 0.001 * (i as f64 + 1.0),
            latency_p50_ms: 100,
            quality_score: 0.5,
        })
        .collect();
    let router = Arc::new(ModelRouter::new(vec![], registry, "c1").with_tiers(tiers));
    Tier {
        fallback: TierFallbackProvider::new(
            primary.0 as Arc<dyn ModelProvider>,
            router,
            RoutePolicy::Cheapest,
        ),
        primary_calls: primary.1,
        c1_calls: c1.1,
        c2_calls: c2.1,
    }
}

fn good(id: &str, text: &str) -> (Arc<Scripted>, Arc<AtomicUsize>) {
    Scripted::new(id, vec![ok(vec![tok(text), ProviderEvent::Done])])
}

#[tokio::test]
async fn test_r9_primary_failing_before_output_falls_back_to_a_candidate() {
    let t = tier(
        Scripted::new("primary", vec![ok(vec![err("Overloaded")])]),
        good("c1", "fallback-ok"),
        good("c2", "unused"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![tok("fallback-ok"), ProviderEvent::Done]);
    assert_eq!(
        (
            count(&t.primary_calls),
            count(&t.c1_calls),
            count(&t.c2_calls)
        ),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn test_r9_timeout_and_stream_request_errors_are_transient() {
    for spec in [
        ErrSpec::Timeout,
        ErrSpec::Stream,
        ErrSpec::Http(503, "x"),
        ErrSpec::Http(529, "x"),
    ] {
        let t = tier(
            Scripted::new("primary", vec![Call::Fail(spec.clone())]),
            good("c1", "fallback-ok"),
            good("c2", "unused"),
        );

        let events = events_of(&t.fallback, request()).await;

        assert_eq!(
            events,
            vec![tok("fallback-ok"), ProviderEvent::Done],
            "{spec:?}"
        );
    }
}

#[tokio::test]
async fn test_r9_non_transient_request_error_is_returned_at_once() {
    let t = tier(
        Scripted::new("primary", vec![Call::Fail(ErrSpec::Http(400, "bad"))]),
        good("c1", "x"),
        good("c2", "y"),
    );

    let e = expect_err(&t.fallback, request()).await;

    assert!(
        matches!(e, ProviderError::Http { status: 400, .. }),
        "{e:?}"
    );
    assert_eq!(count(&t.c1_calls), 0);
}

#[tokio::test]
async fn test_r9_usage_of_the_failed_primary_is_carried_forward() {
    let t = tier(
        Scripted::new("primary", vec![ok(vec![usage(10), err("Overloaded")])]),
        good("c1", "ok"),
        good("c2", "unused"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![usage(10), tok("ok"), ProviderEvent::Done]);
}

#[tokio::test]
async fn test_r9_every_provider_failing_before_output_uses_the_hops_and_reports_the_last_failure() {
    let t = tier(
        Scripted::new("primary", vec![ok(vec![err("primary-failed")])]),
        Scripted::new("c1", vec![ok(vec![err("c1-failed")])]),
        Scripted::new("c2", vec![ok(vec![err("c2-failed")])]),
    );
    let fallback = t.fallback.with_max_hops(1);

    let events = events_of(&fallback, request()).await;

    assert_eq!(
        events,
        vec![err("c1-failed")],
        "the last attempted provider's failure"
    );
    assert_eq!(
        (
            count(&t.primary_calls),
            count(&t.c1_calls),
            count(&t.c2_calls)
        ),
        (1, 1, 0),
        "primary + max_hops = 2 calls"
    );
}

#[tokio::test]
async fn test_r9_failure_after_output_is_not_moved_to_a_candidate() {
    let t = tier(
        Scripted::new("primary", vec![ok(vec![tok("partial"), err("cut")])]),
        good("c1", "x"),
        good("c2", "y"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![tok("partial"), err("cut")]);
    assert_eq!(count(&t.c1_calls), 0);
}

#[tokio::test]
async fn test_r9_primary_that_runs_its_own_tools_is_not_retried_after_its_stream_started() {
    let t = tier(
        Scripted::own_tools("primary", vec![ok(vec![err("cli-failed")])]),
        good("c1", "x"),
        good("c2", "y"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![err("cli-failed")]);
    assert_eq!(count(&t.c1_calls), 0);
}

#[tokio::test]
async fn test_r9_healthy_primary_streams_through_untouched() {
    let t = tier(good("primary", "fine"), good("c1", "x"), good("c2", "y"));

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![tok("fine"), ProviderEvent::Done]);
    assert_eq!(count(&t.c1_calls), 0);
}

// -- R12: an idle timeout is not handed to another provider -------------------------------------------------

const TIMED_OUT: &str = "openai response timed out: operation timed out";
const INTERRUPTED: &str = "openai response interrupted: connection reset";

#[tokio::test]
async fn test_r12_chain_passes_an_idle_timeout_on_without_trying_the_next_member() {
    let (a, a_calls) = Scripted::new(
        "a",
        vec![ok(vec![err_kind(TIMED_OUT, StreamErrorKind::IdleTimeout)])],
    );
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![err_kind(TIMED_OUT, StreamErrorKind::IdleTimeout)]
    );
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 0));
}

#[tokio::test]
async fn test_r12_chain_control_an_interrupted_response_is_still_handed_over() {
    let (a, _) = Scripted::new("a", vec![ok(vec![err(INTERRUPTED)])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![mf("a", "b", INTERRUPTED), tok("b"), ProviderEvent::Done]
    );
    assert_eq!(count(&b_calls), 1);
}

#[tokio::test]
async fn test_r12_tier_fallback_passes_an_idle_timeout_on_without_using_a_hop() {
    let t = tier(
        Scripted::new(
            "primary",
            vec![ok(vec![err_kind(TIMED_OUT, StreamErrorKind::IdleTimeout)])],
        ),
        good("c1", "x"),
        good("c2", "y"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(
        events,
        vec![err_kind(TIMED_OUT, StreamErrorKind::IdleTimeout)]
    );
    assert_eq!((count(&t.primary_calls), count(&t.c1_calls)), (1, 0));
}

#[tokio::test]
async fn test_r12_tier_fallback_control_an_interrupted_response_still_uses_a_hop() {
    let t = tier(
        Scripted::new("primary", vec![ok(vec![err(INTERRUPTED)])]),
        good("c1", "fallback-ok"),
        good("c2", "y"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![tok("fallback-ok"), ProviderEvent::Done]);
}

// -- E6 / E7: the kind decides, and survives -----------------------------------------------------------------------

fn prov(status: Option<u16>) -> StreamErrorKind {
    StreamErrorKind::Provider { status }
}

#[tokio::test]
async fn test_e6_chain_passes_a_non_retryable_provider_error_on_with_its_kind() {
    for status in [400u16, 401, 403, 404, 413] {
        let failure = err_kind("rejected", prov(Some(status)));
        let (a, a_calls) = Scripted::new("a", vec![ok(vec![failure.clone()])]);
        let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

        let events = events_of(&chain(vec![a, b]), request()).await;

        assert_eq!(
            events,
            vec![failure],
            "status {status}: A's whole event, kind included"
        );
        assert_eq!(
            (count(&a_calls), count(&b_calls)),
            (1, 0),
            "status {status}"
        );
    }
}

#[tokio::test]
async fn test_e6_chain_hands_over_a_retryable_failure_of_any_kind() {
    for kind in [
        prov(Some(503)),
        prov(Some(529)),
        prov(Some(429)),
        prov(None),
        StreamErrorKind::Interrupted,
        StreamErrorKind::Unclassified,
    ] {
        let (a, _) = Scripted::new("a", vec![ok(vec![err_kind("failed", kind)])]);
        let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

        let events = events_of(&chain(vec![a, b]), request()).await;

        assert_eq!(
            events,
            vec![mf("a", "b", "failed"), tok("b"), ProviderEvent::Done],
            "{kind:?}"
        );
        assert_eq!(count(&b_calls), 1, "{kind:?}");
    }
}

#[tokio::test]
async fn test_e6_an_unclassified_error_that_looks_like_an_idle_timeout_is_handed_over() {
    // The text check is gone: only the kind exempts an idle timeout.
    let (a, _) = Scripted::new("a", vec![ok(vec![err(TIMED_OUT)])]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(
        events,
        vec![mf("a", "b", TIMED_OUT), tok("b"), ProviderEvent::Done]
    );
    assert_eq!(count(&b_calls), 1);
}

#[tokio::test]
async fn test_e6_tier_fallback_follows_the_kind_too() {
    // non-retryable: passed on, no hop
    let failure = err_kind("rejected", prov(Some(401)));
    let t = tier(
        Scripted::new("primary", vec![ok(vec![failure.clone()])]),
        good("c1", "x"),
        good("c2", "y"),
    );
    assert_eq!(events_of(&t.fallback, request()).await, vec![failure]);
    assert_eq!(count(&t.c1_calls), 0);

    // retryable provider status: hop
    let t = tier(
        Scripted::new("primary", vec![ok(vec![err_kind("down", prov(Some(503)))])]),
        good("c1", "fallback-ok"),
        good("c2", "y"),
    );
    assert_eq!(
        events_of(&t.fallback, request()).await,
        vec![tok("fallback-ok"), ProviderEvent::Done]
    );

    // unclassified idle-timeout-looking text: hop
    let t = tier(
        Scripted::new("primary", vec![ok(vec![err(TIMED_OUT)])]),
        good("c1", "fallback-ok"),
        good("c2", "y"),
    );
    assert_eq!(
        events_of(&t.fallback, request()).await,
        vec![tok("fallback-ok"), ProviderEvent::Done]
    );
}

#[tokio::test]
async fn test_e7_chain_reports_the_last_failure_with_its_kind() {
    let (a, _) = Scripted::new(
        "a",
        vec![ok(vec![
            usage(10),
            err_kind("a-failed", StreamErrorKind::Interrupted),
        ])],
    );
    let (b, _) = Scripted::new("b", vec![ok(vec![err_kind("b-failed", prov(None))])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    assert_eq!(events, vec![usage(10), err_kind("b-failed", prov(None))]);
}

#[tokio::test]
async fn test_e7_tier_fallback_reports_the_last_failure_with_its_kind() {
    let t = tier(
        Scripted::new(
            "primary",
            vec![ok(vec![err_kind(
                "primary-failed",
                StreamErrorKind::Interrupted,
            )])],
        ),
        Scripted::new("c1", vec![ok(vec![err_kind("c1-failed", prov(Some(503)))])]),
        good("c2", "y"),
    );
    let fallback = t.fallback.with_max_hops(1);

    let events = events_of(&fallback, request()).await;

    assert_eq!(events, vec![err_kind("c1-failed", prov(Some(503)))]);
}

#[tokio::test]
async fn test_e7_the_probe_replays_a_failure_with_its_kind() {
    use hone_providers::{Probe, probe_first_output};
    for kind in [
        StreamErrorKind::Interrupted,
        StreamErrorKind::IdleTimeout,
        prov(Some(400)),
        prov(None),
        StreamErrorKind::Unclassified,
    ] {
        let original = vec![usage(5), err_kind("failed", kind)];
        let stream: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> =
            Box::pin(stream::iter(original.clone()));

        match probe_first_output(stream).await {
            Probe::Failed(f) => assert_eq!(f.into_events(), original, "{kind:?}"),
            Probe::Stream(_) => panic!("{kind:?}: expected a failure before output"),
        }
    }
}

#[tokio::test]
async fn test_e7_caching_provider_passes_the_kind_through() {
    use hone_providers::CachingProvider;
    let failure = err_kind("failed", prov(Some(400)));
    let (inner, _) = Scripted::new("inner", vec![ok(vec![tok("par"), failure.clone()])]);
    let cache = CachingProvider::new(inner as Arc<dyn ModelProvider>, 10);

    let events = events_of(&cache, request()).await;

    assert_eq!(events, vec![tok("par"), failure]);
}

// -- G5: no answer at all moves on ---------------------------------------------------------------------------

#[tokio::test]
async fn test_g5_chain_moves_on_when_a_member_got_no_response() {
    let (a, a_calls) = Scripted::new(
        "a",
        vec![Call::Fail(ErrSpec::Http(
            0,
            "error sending request for url (http://x/)",
        ))],
    );
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let events = events_of(&chain(vec![a, b]), request()).await;

    let reason = ProviderError::Http {
        provider: "a".to_owned(),
        status: 0,
        body: "error sending request for url (http://x/)".to_owned(),
    }
    .to_string();
    assert_eq!(
        events,
        vec![mf("a", "b", &reason), tok("b"), ProviderEvent::Done]
    );
    assert_eq!((count(&a_calls), count(&b_calls)), (1, 1));
}

#[tokio::test]
async fn test_g5_tier_fallback_moves_on_when_the_primary_got_no_response() {
    let t = tier(
        Scripted::new(
            "primary",
            vec![Call::Fail(ErrSpec::Http(0, "error sending request"))],
        ),
        good("c1", "fallback-ok"),
        good("c2", "unused"),
    );

    let events = events_of(&t.fallback, request()).await;

    assert_eq!(events, vec![tok("fallback-ok"), ProviderEvent::Done]);
    assert_eq!(count(&t.c1_calls), 1);
}

#[tokio::test]
async fn test_g5_a_client_error_still_stops_the_chain() {
    let (a, _) = Scripted::new("a", vec![Call::Fail(ErrSpec::Http(401, "bad key"))]);
    let (b, b_calls) = Scripted::new("b", vec![ok(vec![tok("b"), ProviderEvent::Done])]);

    let e = expect_err(&chain(vec![a, b]), request()).await;

    assert!(
        matches!(e, ProviderError::Http { status: 401, .. }),
        "{e:?}"
    );
    assert_eq!(count(&b_calls), 0);
}
