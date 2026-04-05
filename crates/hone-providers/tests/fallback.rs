use std::sync::Arc;

use futures::StreamExt;
use hone_proto::{ModelId, ProviderEvent};
use hone_providers::fallback::{RateLimitedProvider, UnavailableProvider};
use hone_providers::{CompletionRequest, FallbackChain, MockProvider, ModelProvider, ProviderError};

fn request() -> CompletionRequest {
    CompletionRequest::new(ModelId::new("mock:mock-model"), vec![])
}

#[tokio::test]
async fn test_fallback_uses_first_healthy() {
    let first: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["ok".into()]).with_id("first"));
    let second: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["should not appear".into()]).with_id("second"));

    let chain = FallbackChain::new(vec![first, second]);
    let stream = chain.complete(request()).await.unwrap();

    let events: Vec<_> = stream.collect().await;
    assert!(events.contains(&ProviderEvent::Token { text: "ok".into() }));
    assert!(!events.contains(&ProviderEvent::Token {
        text: "should not appear".into()
    }));
}

#[tokio::test]
async fn test_fallback_tries_next_on_rate_limit() {
    let first: Arc<dyn ModelProvider> =
        Arc::new(RateLimitedProvider { id: "rate-limited".into() });
    let second: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["fallback-token".into()]).with_id("backup"));

    let chain = FallbackChain::new(vec![first, second]);
    let stream = chain.complete(request()).await.unwrap();

    let events: Vec<_> = stream.collect().await;
    assert!(
        events.contains(&ProviderEvent::Token { text: "fallback-token".into() }),
        "expected fallback provider token, got {events:?}"
    );
}

#[tokio::test]
async fn test_fallback_all_fail_returns_last_error() {
    let first: Arc<dyn ModelProvider> =
        Arc::new(UnavailableProvider { id: "offline-1".into() });
    let second: Arc<dyn ModelProvider> =
        Arc::new(UnavailableProvider { id: "offline-2".into() });

    let chain = FallbackChain::new(vec![first, second]);
    let result = chain.complete(request()).await;

    assert!(result.is_err(), "expected error when all providers fail");
    let err = result.err().unwrap();
    match err {
        ProviderError::Http { status, .. } => assert_eq!(status, 503),
        other => panic!("unexpected error variant: {other:?}"),
    }
}
