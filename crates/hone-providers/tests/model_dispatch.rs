#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G3 and G4 (`docs/plans/routing-fallback-spec.md`): `ModelDispatch` forwards each request to
//! the provider for its model's prefix, and with fallback models hands a failed request to the
//! next model's provider (same rules as `FallbackChain`).
//!
//! API (spec section 3): `ModelDispatch::new(default_model)`, `.with_provider(prefix, provider)`,
//! `.with_fallback(Vec<ModelId>)`, `.has_provider_for(&ModelId)`, `impl ModelProvider`.
//! Own file: it does not compile until `ModelDispatch` exists.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{
    Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, StreamErrorKind, TokenUsage,
};
use hone_providers::{CompletionRequest, ModelDispatch, ModelProvider, ProviderError};

// -- recording provider ------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Call {
    Events(Vec<ProviderEvent>),
    Http(u16, &'static str),
}

struct Rec {
    id: String,
    script: Vec<Call>,
    own_tools: bool,
    calls: AtomicUsize,
    seen: Mutex<Vec<(String, Vec<String>)>>, // (model, message texts) per request
}

impl Rec {
    fn new(id: &str, script: Vec<Call>) -> Arc<Self> {
        Self::build(id, script, false)
    }
    fn build(id: &str, script: Vec<Call>, own_tools: bool) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_owned(),
            script,
            own_tools,
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn models_seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }
}

#[async_trait]
impl ModelProvider for Rec {
    fn runs_own_tools(&self) -> bool {
        self.own_tools
    }
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.id
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new(format!("{}:m", self.id)),
            display_name: self.id.clone(),
            context_length: 1000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let n = self
            .calls
            .fetch_add(1, Ordering::SeqCst)
            .min(self.script.len() - 1);
        self.seen.lock().unwrap().push((
            r.model.as_str().to_owned(),
            r.messages.iter().map(|m| m.content.as_text()).collect(),
        ));
        match self.script[n].clone() {
            Call::Events(e) => Ok(Box::pin(stream::iter(e))),
            Call::Http(status, body) => Err(ProviderError::Http {
                provider: self.id.clone(),
                status,
                body: body.to_owned(),
            }),
        }
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        7
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn tok(t: &str) -> ProviderEvent {
    ProviderEvent::Token { text: t.to_owned() }
}
fn done() -> ProviderEvent {
    ProviderEvent::Done
}
fn err(m: &str, kind: StreamErrorKind) -> ProviderEvent {
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
fn answer(t: &str) -> Call {
    Call::Events(vec![tok(t), done()])
}
fn id(s: &str) -> ModelId {
    ModelId::new(s)
}
fn req(model: &str) -> CompletionRequest {
    CompletionRequest::new(id(model), vec![Message::user("the prompt")])
}

async fn events(p: &dyn ModelProvider, r: CompletionRequest) -> Vec<ProviderEvent> {
    let s = tokio::time::timeout(Duration::from_secs(10), p.complete(r))
        .await
        .expect("complete hung")
        .expect("a stream");
    tokio::time::timeout(Duration::from_secs(10), s.collect::<Vec<_>>())
        .await
        .expect("stream hung")
}

async fn request_error(p: &dyn ModelProvider, r: CompletionRequest) -> ProviderError {
    match tokio::time::timeout(Duration::from_secs(10), p.complete(r))
        .await
        .expect("hung")
    {
        Err(e) => e,
        Ok(_) => panic!("expected a request error"),
    }
}

struct Set {
    dispatch: ModelDispatch,
    anthropic: Arc<Rec>,
    openai: Arc<Rec>,
    ollama: Arc<Rec>,
}

fn set(default: &str, a: Vec<Call>, o: Vec<Call>, l: Vec<Call>) -> Set {
    let anthropic = Rec::new("anthropic", a);
    let openai = Rec::new("openai", o);
    let ollama = Rec::new("ollama", l);
    let dispatch = ModelDispatch::new(id(default))
        .with_provider(
            "anthropic",
            Arc::clone(&anthropic) as Arc<dyn ModelProvider>,
        )
        .with_provider("openai", Arc::clone(&openai) as Arc<dyn ModelProvider>)
        .with_provider("ollama", Arc::clone(&ollama) as Arc<dyn ModelProvider>);
    Set {
        dispatch,
        anthropic,
        openai,
        ollama,
    }
}

// -- G3 ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g3_requests_reach_the_provider_for_their_prefix_unchanged() {
    for (model, expect) in [
        ("anthropic:claude-x", "anthropic"),
        ("openai:gpt-x", "openai"),
        ("ollama:llama-x", "ollama"),
    ] {
        let s = set(
            "openai:gpt-x",
            vec![answer("a")],
            vec![answer("o")],
            vec![answer("l")],
        );

        let ev = events(&s.dispatch, req(model)).await;

        let answers = match expect {
            "anthropic" => "a",
            "openai" => "o",
            _ => "l",
        };
        assert_eq!(ev, vec![tok(answers), done()], "{model}");
        let counts = [s.anthropic.calls(), s.openai.calls(), s.ollama.calls()];
        let want = match expect {
            "anthropic" => [1, 0, 0],
            "openai" => [0, 1, 0],
            _ => [0, 0, 1],
        };
        assert_eq!(counts, want, "{model}");
        let seen = match expect {
            "anthropic" => &s.anthropic,
            "openai" => &s.openai,
            _ => &s.ollama,
        };
        assert_eq!(
            seen.models_seen(),
            vec![model.to_owned()],
            "the model id reaches the provider unchanged"
        );
        assert_eq!(
            seen.seen.lock().unwrap()[0].1,
            vec!["the prompt".to_owned()]
        );
    }
}

#[tokio::test]
async fn test_g3_a_prefix_without_a_provider_is_model_not_found_and_nothing_is_called() {
    let openai = Rec::new("openai", vec![answer("o")]);
    let d = ModelDispatch::new(id("openai:gpt-x"))
        .with_provider("openai", Arc::clone(&openai) as Arc<dyn ModelProvider>);

    let e = request_error(&d, req("ollama:llama-x")).await;

    assert!(matches!(e, ProviderError::ModelNotFound { .. }), "{e:?}");
    assert_eq!(openai.calls(), 0);
}

#[test]
fn test_g3_has_provider_for_follows_the_registered_prefixes() {
    let openai = Rec::new("openai", vec![answer("o")]);
    let d = ModelDispatch::new(id("openai:gpt-x"))
        .with_provider("openai", openai as Arc<dyn ModelProvider>);
    assert!(d.has_provider_for(&id("openai:anything")));
    assert!(!d.has_provider_for(&id("anthropic:claude")));
    assert!(!d.has_provider_for(&id("ollama:x")));
    assert!(!d.has_provider_for(&id("no-prefix")));
}

#[tokio::test]
async fn test_g3_identity_and_metadata_are_the_default_models_providers() {
    let s = set(
        "anthropic:claude-x",
        vec![answer("a")],
        vec![answer("o")],
        vec![answer("l")],
    );
    assert_eq!(s.dispatch.id(), "anthropic");
    assert_eq!(s.dispatch.display_name(), "anthropic");
    assert_eq!(s.dispatch.estimate_tokens(&[Message::user("x")]), 7);
    assert_eq!(s.dispatch.health().await, ProviderHealth::Healthy);
    assert_eq!(s.dispatch.supported_models()[0].id.as_str(), "anthropic:m");

    let s = set(
        "ollama:llama-x",
        vec![answer("a")],
        vec![answer("o")],
        vec![answer("l")],
    );
    assert_eq!(s.dispatch.id(), "ollama");
}

#[test]
fn test_g3_runs_own_tools_is_true_if_any_member_does() {
    let plain = Rec::new("openai", vec![answer("o")]);
    let cli = Rec::build("ollama", vec![answer("l")], true);
    let d = ModelDispatch::new(id("openai:x"))
        .with_provider("openai", plain as Arc<dyn ModelProvider>)
        .with_provider("ollama", Arc::clone(&cli) as Arc<dyn ModelProvider>);
    assert!(d.runs_own_tools());

    let plain2 = Rec::new("openai", vec![answer("o")]);
    let d = ModelDispatch::new(id("openai:x"))
        .with_provider("openai", plain2 as Arc<dyn ModelProvider>);
    assert!(!d.runs_own_tools());
}

#[tokio::test]
async fn test_g3_without_fallback_models_a_failure_is_passed_on() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![err(
            "boom",
            StreamErrorKind::Interrupted,
        )])],
        vec![answer("l")],
    );

    let ev = events(&s.dispatch, req("openai:gpt-x")).await;

    assert_eq!(ev, vec![err("boom", StreamErrorKind::Interrupted)]);
    assert_eq!(s.ollama.calls(), 0);
}

// -- G4 ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g4_a_failure_before_output_goes_to_the_fallback_model_with_its_id() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![err(
            "overloaded",
            StreamErrorKind::Interrupted,
        )])],
        vec![answer("fallback-ok")],
    );
    let d = s.dispatch.with_fallback(vec![id("ollama:fb-model")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "ollama:fb-model", "overloaded"),
            tok("fallback-ok"),
            done()
        ],
        "N4: the notice names the full model ids"
    );
    assert_eq!(s.openai.calls(), 1);
    assert_eq!(
        s.ollama.models_seen(),
        vec!["ollama:fb-model".to_owned()],
        "model replaced, rest unchanged"
    );
    assert_eq!(
        s.ollama.seen.lock().unwrap()[0].1,
        vec!["the prompt".to_owned()]
    );
}

#[tokio::test]
async fn test_g4_a_transient_request_error_falls_back_and_a_non_transient_one_does_not() {
    for status in [503u16, 500, 529, 429, 0] {
        let s = set(
            "openai:gpt-x",
            vec![answer("a")],
            vec![Call::Http(status, "down")],
            vec![answer("fb")],
        );
        let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);
        let ev = events(&d, req("openai:gpt-x")).await;
        match ev.first() {
            Some(ProviderEvent::ModelFallback {
                requested,
                fallback,
                reason,
            }) => {
                assert_eq!(
                    (requested.as_str(), fallback.as_str()),
                    ("openai:gpt-x", "ollama:fb"),
                    "status {status}"
                );
                assert!(reason.contains("down"), "status {status}: {reason}");
            }
            other => panic!("status {status}: expected a leading ModelFallback, got {other:?}"),
        }
        assert_eq!(ev[1..], [tok("fb"), done()], "status {status}");
    }
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Http(400, "bad")],
        vec![answer("fb")],
    );
    let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);

    let e = request_error(&d, req("openai:gpt-x")).await;

    assert!(
        matches!(e, ProviderError::Http { status: 400, .. }),
        "{e:?}"
    );
    assert_eq!(s.ollama.calls(), 0);
}

#[tokio::test]
async fn test_g4_a_failure_after_output_is_not_handed_over() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![
            tok("partial"),
            err("cut", StreamErrorKind::Interrupted),
        ])],
        vec![answer("fb")],
    );
    let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![tok("partial"), err("cut", StreamErrorKind::Interrupted)]
    );
    assert_eq!(s.ollama.calls(), 0);
}

#[tokio::test]
async fn test_g4_non_retryable_kinds_are_not_handed_over() {
    for kind in [
        StreamErrorKind::IdleTimeout,
        StreamErrorKind::Provider { status: Some(401) },
    ] {
        let s = set(
            "openai:gpt-x",
            vec![answer("a")],
            vec![Call::Events(vec![err("no", kind)])],
            vec![answer("fb")],
        );
        let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);

        let ev = events(&d, req("openai:gpt-x")).await;

        assert_eq!(ev, vec![err("no", kind)], "{kind:?}");
        assert_eq!(s.ollama.calls(), 0, "{kind:?}");
    }
}

#[tokio::test]
async fn test_g4_a_fallback_equal_to_the_primary_is_skipped() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![err(
            "overloaded",
            StreamErrorKind::Interrupted,
        )])],
        vec![answer("fb")],
    );
    let d = s
        .dispatch
        .with_fallback(vec![id("openai:gpt-x"), id("ollama:fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "ollama:fb", "overloaded"),
            tok("fb"),
            done()
        ]
    );
    assert_eq!(
        s.openai.calls(),
        1,
        "the primary is tried once, not again as its own fallback"
    );
}

#[tokio::test]
async fn test_g4_a_fallback_whose_provider_is_missing_is_skipped() {
    let openai = Rec::new(
        "openai",
        vec![Call::Events(vec![err(
            "overloaded",
            StreamErrorKind::Interrupted,
        )])],
    );
    let ollama = Rec::new("ollama", vec![answer("fb")]);
    let d = ModelDispatch::new(id("openai:gpt-x"))
        .with_provider("openai", Arc::clone(&openai) as Arc<dyn ModelProvider>)
        .with_provider("ollama", Arc::clone(&ollama) as Arc<dyn ModelProvider>)
        .with_fallback(vec![id("anthropic:no-key"), id("ollama:fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "ollama:fb", "overloaded"),
            tok("fb"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_g4_usage_of_a_failed_member_is_carried_forward() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![
            usage(10),
            err("overloaded", StreamErrorKind::Interrupted),
        ])],
        vec![answer("fb")],
    );
    let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "ollama:fb", "overloaded"),
            usage(10),
            tok("fb"),
            done()
        ]
    );
}

#[tokio::test]
async fn test_g4_when_every_model_fails_the_last_failure_is_reported_as_that_member_reported_it() {
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![Call::Events(vec![err(
            "first",
            StreamErrorKind::Interrupted,
        )])],
        vec![Call::Events(vec![err(
            "last",
            StreamErrorKind::Provider { status: None },
        )])],
    );
    let d = s.dispatch.with_fallback(vec![id("ollama:fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![err("last", StreamErrorKind::Provider { status: None })]
    );
    assert_eq!(
        (s.openai.calls(), s.ollama.calls()),
        (1, 1),
        "each model at most once"
    );
}

#[tokio::test]
async fn test_g4_fallbacks_are_tried_in_order_until_one_answers() {
    let s = set(
        "openai:gpt-x",
        vec![answer("anthropic-ok")],
        vec![Call::Events(vec![err(
            "overloaded",
            StreamErrorKind::Interrupted,
        )])],
        vec![Call::Events(vec![err(
            "also down",
            StreamErrorKind::Interrupted,
        )])],
    );
    let d = s
        .dispatch
        .with_fallback(vec![id("ollama:first-fb"), id("anthropic:second-fb")]);

    let ev = events(&d, req("openai:gpt-x")).await;

    assert_eq!(
        ev,
        vec![
            mf("openai:gpt-x", "anthropic:second-fb", "overloaded"),
            tok("anthropic-ok"),
            done()
        ]
    );
    assert_eq!(s.ollama.models_seen(), vec!["ollama:first-fb".to_owned()]);
    assert_eq!(
        s.anthropic.models_seen(),
        vec!["anthropic:second-fb".to_owned()]
    );
}

#[tokio::test]
async fn test_g4_a_request_for_another_model_uses_it_as_the_primary() {
    // Routing sends the request to ollama; the fallback list still applies after it.
    let s = set(
        "openai:gpt-x",
        vec![answer("a")],
        vec![answer("o")],
        vec![Call::Events(vec![err(
            "down",
            StreamErrorKind::Interrupted,
        )])],
    );
    let d = s.dispatch.with_fallback(vec![id("openai:backup")]);

    let ev = events(&d, req("ollama:routed")).await;

    assert_eq!(
        ev,
        vec![
            mf("ollama:routed", "openai:backup", "down"),
            tok("o"),
            done()
        ]
    );
    assert_eq!(s.ollama.models_seen(), vec!["ollama:routed".to_owned()]);
    assert_eq!(s.openai.models_seen(), vec!["openai:backup".to_owned()]);
}
