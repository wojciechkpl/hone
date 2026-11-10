#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for `docs/plans/stream-transport-error-spec.md` (S1-S6): cut-off responses
//! must not look complete; in-band errors surface; an idle limit replaces the 120 s total
//! deadline; failed responses are not cached.
//!
//! API assumed from the spec: `with_idle_timeout(Duration) -> Self` on `OpenAiProvider`,
//! `AnthropicProvider` and `OllamaProvider`; `http_client_timeout()` returns the idle limit
//! (default 120 s). Provider names and wording are asserted loosely (case-insensitive
//! `contains`). Timings are short (idle limit 1 s) with generous outer timeouts.

mod common;

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{
    ADAPTERS, Adapter, End, Step, errors, has_done, head, in_band_error, marker, piece, run, text,
    text_of, trailer,
};
use futures::{Stream, StreamExt, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};
use hone_providers::{
    AnthropicProvider, CachingProvider, CompletionRequest, ModelProvider, OllamaProvider,
    OpenAiProvider, ProviderError,
};

fn lc(s: &str) -> String {
    s.to_lowercase()
}

/// `head` + the given text events as steps.
fn steps_with(a: Adapter, texts: &[&str], tail: &[String]) -> Vec<Step> {
    let mut v = vec![piece(head(a))];
    for t in texts {
        v.push(piece(text(a, t)));
    }
    for t in tail {
        v.push(piece(t.clone()));
    }
    v
}

// -- S1: transport error before completion ------------------------------------------------------

#[tokio::test]
async fn test_s1_body_cut_off_before_its_end_delivers_earlier_events_then_one_error_and_no_done() {
    for a in ADAPTERS {
        let events = run(
            a,
            steps_with(a, &["hello ", "world"], &[]),
            End::CutOff,
            None,
        )
        .await;

        assert_eq!(
            text_of(&events),
            "hello world",
            "{a:?}: events before the cut arrive"
        );
        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: exactly one Error: {events:?}");
        assert!(
            lc(&errs[0]).contains(a.name()),
            "{a:?}: names the provider: {}",
            errs[0]
        );
        assert!(
            lc(&errs[0]).contains("interrupted"),
            "{a:?}: says interrupted: {}",
            errs[0]
        );
        assert!(
            !has_done(&events),
            "{a:?}: no Done after a cut-off: {events:?}"
        );
        assert!(
            matches!(events.last(), Some(ProviderEvent::Error { .. })),
            "{a:?}: the Error ends the stream: {events:?}"
        );
    }
}

// -- S2: clean end before completion ----------------------------------------------------------------

#[tokio::test]
async fn test_s2_clean_end_without_a_completion_marker_is_an_error() {
    for a in ADAPTERS {
        let events = run(a, steps_with(a, &["partial"], &[]), End::Normal, None).await;

        assert_eq!(text_of(&events), "partial", "{a:?}");
        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: exactly one Error: {events:?}");
        assert!(lc(&errs[0]).contains("complete"), "{a:?}: {}", errs[0]);
        assert!(!has_done(&events), "{a:?}: {events:?}");
    }
}

#[tokio::test]
async fn test_s2_control_the_same_body_with_the_marker_ends_without_an_error() {
    for a in ADAPTERS {
        let tail = [marker(a), trailer(a)];
        let events = run(a, steps_with(a, &["partial"], &tail), End::Normal, None).await;

        assert_eq!(text_of(&events), "partial", "{a:?}");
        assert!(
            errors(&events).is_empty(),
            "{a:?}: no Error when complete: {events:?}"
        );
        assert!(has_done(&events), "{a:?}: {events:?}");
    }
}

// -- S3: completion wins -------------------------------------------------------------------------------

#[tokio::test]
async fn test_s3_a_cut_after_the_completion_marker_is_not_an_error() {
    for a in ADAPTERS {
        // OpenAI: finish_reason chunk seen, socket closed before [DONE].
        // Anthropic: message_stop seen, then the connection is cut. Ollama: done line, then cut.
        let tail = [marker(a)];
        let events = run(a, steps_with(a, &["all of it"], &tail), End::CutOff, None).await;

        assert_eq!(text_of(&events), "all of it", "{a:?}");
        assert!(
            errors(&events).is_empty(),
            "{a:?}: completion wins over a later cut: {events:?}"
        );
        assert!(has_done(&events), "{a:?}: {events:?}");
    }
}

// -- S4: in-band errors ----------------------------------------------------------------------------------

#[tokio::test]
async fn test_s4_in_band_error_event_is_reported_once_and_stops_the_reader() {
    for a in ADAPTERS {
        // The error is followed by what would be a normal completion: it must not become Done.
        let tail = [in_band_error(a, "Overloaded"), marker(a), trailer(a)];
        let events = run(a, steps_with(a, &["partial"], &tail), End::Normal, None).await;

        assert_eq!(
            text_of(&events),
            "partial",
            "{a:?}: events before the error are delivered"
        );
        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: exactly one Error: {events:?}");
        assert!(
            errs[0].contains("Overloaded"),
            "{a:?}: carries the provider's message: {}",
            errs[0]
        );
        assert!(
            !has_done(&events),
            "{a:?}: the reader stops after an error: {events:?}"
        );
    }
}

#[tokio::test]
async fn test_s4_openrouter_shape_error_with_finish_reason_error_then_done_marker() {
    let a = Adapter::OpenAi;
    let chunk = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}],
        "error": {"message": "Provider disconnected unexpectedly", "code": 502}
    });
    let tail = [format!("data: {chunk}\n\n"), "data: [DONE]\n\n".to_owned()];
    let events = run(a, steps_with(a, &["partial"], &tail), End::Normal, None).await;

    assert_eq!(text_of(&events), "partial");
    let errs = errors(&events);
    assert_eq!(errs.len(), 1, "{events:?}");
    assert!(
        errs[0].contains("Provider disconnected unexpectedly"),
        "{}",
        errs[0]
    );
    assert!(
        !has_done(&events),
        "[DONE] after an error must not turn it into Done: {events:?}"
    );
}

#[tokio::test]
async fn test_s4_top_level_error_object_without_a_choices_array() {
    let a = Adapter::OpenAi;
    let tail = [in_band_error(a, "Rate limit exceeded")];
    let events = run(a, steps_with(a, &[], &tail), End::Normal, None).await;
    let errs = errors(&events);
    assert_eq!(errs.len(), 1, "{events:?}");
    assert!(errs[0].contains("Rate limit exceeded"), "{}", errs[0]);
}

// -- S5: idle limit, not a total deadline ------------------------------------------------------------------

const IDLE: Duration = Duration::from_secs(1);

#[tokio::test]
async fn test_s5_a_response_that_keeps_streaming_is_never_cut_off_by_the_idle_limit() {
    for a in ADAPTERS {
        // About 3 s of streaming (a piece every 200 ms) against a 1 s idle limit.
        let mut steps = vec![piece(head(a))];
        let mut expected = String::new();
        for i in 0..15 {
            let t = format!("t{i} ");
            expected.push_str(&t);
            steps.push(piece(text(a, &t)));
            steps.push(Step::Delay(Duration::from_millis(200)));
        }
        steps.push(piece(marker(a)));
        steps.push(piece(trailer(a)));

        let started = Instant::now();
        let events = run(a, steps, End::Normal, Some(IDLE)).await;

        assert!(
            started.elapsed() > Duration::from_secs(2),
            "{a:?}: the body really streamed for a while"
        );
        assert_eq!(text_of(&events), expected, "{a:?}");
        assert!(
            errors(&events).is_empty(),
            "{a:?}: a live stream is not idle: {events:?}"
        );
        assert!(has_done(&events), "{a:?}: {events:?}");
    }
}

#[tokio::test]
async fn test_s5_a_stall_longer_than_the_idle_limit_is_a_timed_out_error() {
    for a in ADAPTERS {
        let mut steps = vec![piece(head(a)), piece(text(a, "first"))];
        steps.push(Step::Delay(Duration::from_secs(2)));
        steps.push(piece(text(a, "late")));
        steps.push(piece(marker(a)));
        steps.push(piece(trailer(a)));

        let started = Instant::now();
        let events = run(a, steps, End::Normal, Some(IDLE)).await;

        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{a:?}: gave up in a few seconds"
        );
        let errs = errors(&events);
        assert_eq!(errs.len(), 1, "{a:?}: {events:?}");
        assert!(lc(&errs[0]).contains("timed out"), "{a:?}: {}", errs[0]);
        assert!(lc(&errs[0]).contains(a.name()), "{a:?}: {}", errs[0]);
        assert_eq!(
            text_of(&events),
            "first",
            "{a:?}: the late event is not delivered"
        );
        assert!(!has_done(&events), "{a:?}");
    }
}

#[test]
fn test_s5_http_client_timeout_reports_the_idle_limit_default_120s() {
    assert_eq!(
        OpenAiProvider::new("k").http_client_timeout(),
        Some(Duration::from_secs(120))
    );
    assert_eq!(
        AnthropicProvider::new("k").http_client_timeout(),
        Some(Duration::from_secs(120))
    );
    assert_eq!(
        OllamaProvider::new("http://127.0.0.1:1").http_client_timeout(),
        Some(Duration::from_secs(120))
    );

    let d = Duration::from_secs(7);
    assert_eq!(
        OpenAiProvider::new("k")
            .with_idle_timeout(d)
            .http_client_timeout(),
        Some(d)
    );
    assert_eq!(
        AnthropicProvider::new("k")
            .with_idle_timeout(d)
            .http_client_timeout(),
        Some(d)
    );
    assert_eq!(
        OllamaProvider::new("http://127.0.0.1:1")
            .with_idle_timeout(d)
            .http_client_timeout(),
        Some(d)
    );
}

// -- S6: failed responses are not cached ------------------------------------------------------------------------

/// Call 1 yields `[Token, Error]`; every later call yields `[Token, Done]`.
struct FlakyThenFine {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ModelProvider for FlakyThenFine {
    fn id(&self) -> &str {
        "flaky"
    }
    fn display_name(&self) -> &str {
        "Flaky"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test".to_owned(),
            context_length: 4096,
            supports_tools: false,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if n == 0 {
            vec![
                ProviderEvent::Token {
                    text: "par".to_owned(),
                },
                ProviderEvent::Error {
                    message: "interrupted".to_owned(),
                    kind: hone_proto::StreamErrorKind::Unclassified,
                },
            ]
        } else {
            vec![
                ProviderEvent::Token {
                    text: "full answer".to_owned(),
                },
                ProviderEvent::Done,
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        1
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

#[tokio::test]
async fn test_s6_caching_provider_does_not_store_a_response_that_contains_an_error() {
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CachingProvider::new(
        Arc::new(FlakyThenFine {
            calls: Arc::clone(&calls),
        }),
        10,
    );
    let req = || CompletionRequest::new("mock:test", vec![Message::user("same request")]);
    let collect = |s: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>| async move {
        tokio::time::timeout(Duration::from_secs(10), s.collect::<Vec<_>>())
            .await
            .expect("hung")
    };

    let first = collect(cache.complete(req()).await.unwrap()).await;
    assert!(
        first
            .iter()
            .any(|e| matches!(e, ProviderEvent::Error { .. })),
        "{first:?}"
    );

    let second = collect(cache.complete(req()).await.unwrap()).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the failed response must not have been cached"
    );
    assert!(
        has_done(&second) && errors(&second).is_empty(),
        "{second:?}"
    );

    let third = collect(cache.complete(req()).await.unwrap()).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the good response is cached"
    );
    assert_eq!(text_of(&third), "full answer");
}

#[tokio::test]
async fn test_s6_control_caching_provider_still_caches_a_successful_response() {
    struct AlwaysFine(Arc<AtomicUsize>);
    #[async_trait]
    impl ModelProvider for AlwaysFine {
        fn id(&self) -> &str {
            "fine"
        }
        fn display_name(&self) -> &str {
            "Fine"
        }
        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            _r: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(stream::iter(vec![
                ProviderEvent::Token {
                    text: "ok".to_owned(),
                },
                ProviderEvent::Done,
            ])))
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            1
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::Healthy
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let cache = CachingProvider::new(Arc::new(AlwaysFine(Arc::clone(&calls))), 10);
    for _ in 0..2 {
        let s = cache
            .complete(CompletionRequest::new(
                "mock:test",
                vec![Message::user("x")],
            ))
            .await
            .unwrap();
        let _ = s.collect::<Vec<_>>().await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// -- S8: every finish reason completes the response ----------------------------------------------------

fn openai_finish_chunk(reason: &str) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]})
    )
}

#[tokio::test]
async fn test_s8_openai_any_non_error_finish_reason_completes_the_response() {
    let a = Adapter::OpenAi;
    for reason in [
        "content_filter",
        "function_call",
        "eos_token",
        "some_future_reason",
    ] {
        for end in [End::Normal, End::CutOff] {
            // No `[DONE]` after the finish_reason chunk. Whether a Done is emitted is up to the
            // implementation; what must not happen is an Error.
            let tail = [openai_finish_chunk(reason)];
            let events = run(a, steps_with(a, &["kept text"], &tail), end, None).await;

            assert_eq!(
                text_of(&events),
                "kept text",
                "{reason} / {end:?}: text delivered"
            );
            assert!(
                errors(&events).is_empty(),
                "{reason} / {end:?}: a finish_reason completes the response: {events:?}"
            );
        }
    }
}

#[tokio::test]
async fn test_s8_anthropic_message_delta_with_a_stop_reason_completes_the_response() {
    let a = Adapter::Anthropic;
    // A proxy that drops `message_stop`: head, text, content_block_stop, message_delta only.
    let tail = [
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_owned(),
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n".to_owned(),
    ];
    for end in [End::Normal, End::CutOff] {
        let events = run(a, steps_with(a, &["kept text"], &tail), end, None).await;

        assert_eq!(text_of(&events), "kept text", "{end:?}");
        assert!(
            errors(&events).is_empty(),
            "{end:?}: stop_reason in message_delta completes the response: {events:?}"
        );
    }
}

#[tokio::test]
async fn test_s8_guard_message_delta_without_a_stop_reason_does_not_complete() {
    let a = Adapter::Anthropic;
    let tail = [
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null},\"usage\":{\"output_tokens\":1}}\n\n".to_owned(),
    ];
    let events = run(a, steps_with(a, &["partial"], &tail), End::Normal, None).await;
    assert_eq!(
        errors(&events).len(),
        1,
        "a null stop_reason is not a completion: {events:?}"
    );
}

#[tokio::test]
async fn test_s8_guard_finish_reason_error_is_still_an_error_and_not_a_completion() {
    let a = Adapter::OpenAi;
    for end in [End::Normal, End::CutOff] {
        // finish_reason "error" without an error object, no [DONE].
        let tail = [openai_finish_chunk("error")];
        let events = run(a, steps_with(a, &["partial"], &tail), end, None).await;

        assert_eq!(text_of(&events), "partial", "{end:?}");
        assert_eq!(
            errors(&events).len(),
            1,
            "{end:?}: exactly one Error: {events:?}"
        );
        assert!(!has_done(&events), "{end:?}: {events:?}");
    }
}
