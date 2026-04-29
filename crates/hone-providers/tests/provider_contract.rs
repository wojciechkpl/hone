#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent, ProviderHealth, TokenUsage};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider};

fn simple_request() -> CompletionRequest {
    CompletionRequest::new(
        ModelId::new("mock:mock-model"),
        vec![Message::user("hello")],
    )
}

#[tokio::test]
async fn test_mock_provider_returns_tokens() {
    let provider = MockProvider::new(vec!["Hello".into(), " world".into()]);
    let mut stream = provider.complete(simple_request()).await.unwrap();

    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }

    assert_eq!(
        events,
        vec![
            ProviderEvent::Token {
                text: "Hello".into()
            },
            ProviderEvent::Token {
                text: " world".into()
            },
            ProviderEvent::Done,
        ]
    );
}

#[tokio::test]
async fn test_mock_provider_with_tool_call() {
    let args = serde_json::json!({"path": "/tmp/file.txt"});
    let provider = MockProvider::with_tool_call("call-1", "read_file", args.clone());
    let mut stream = provider.complete(simple_request()).await.unwrap();

    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }

    assert_eq!(events.len(), 2);
    match &events[0] {
        ProviderEvent::ToolCall(req) => {
            assert_eq!(req.id, "call-1");
            assert_eq!(req.name, "read_file");
            assert_eq!(req.arguments, args);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
    assert_eq!(events[1], ProviderEvent::Done);
}

#[tokio::test]
async fn test_mock_provider_usage_event() {
    let usage = TokenUsage {
        input: 10,
        output: 20,
        cache_read: 0,
        cache_creation: 0,
    };
    let provider = MockProvider::new(vec!["hi".into()]).with_usage(usage);
    let stream = provider.complete(simple_request()).await.unwrap();

    let events: Vec<_> = stream.collect().await;

    let usage_event = events.iter().find(|e| matches!(e, ProviderEvent::Usage(_)));
    match usage_event {
        Some(ProviderEvent::Usage(u)) => {
            assert_eq!(u.input, 10);
            assert_eq!(u.output, 20);
        }
        _ => panic!("expected Usage event"),
    }
    assert!(events.contains(&ProviderEvent::Done));
}

#[tokio::test]
async fn test_mock_provider_health_is_healthy() {
    let provider = MockProvider::new(vec![]);
    assert_eq!(provider.health().await, ProviderHealth::Healthy);
}

#[tokio::test]
async fn test_mock_provider_estimate_tokens() {
    let provider = MockProvider::new(vec![]);
    let messages = vec![Message::user(
        "This is a test message with some words in it.",
    )];
    let estimate = provider.estimate_tokens(&messages);
    // Rough lower bound: non-zero for non-trivial input.
    assert!(estimate > 0, "estimate should be positive, got {estimate}");
}
