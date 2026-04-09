use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{AnthropicProvider, CompletionRequest, ModelProvider};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request() -> CompletionRequest {
    CompletionRequest::new(
        ModelId::new("anthropic:claude-sonnet-4-5"),
        vec![Message::user("hello")],
    )
}

// ---------------------------------------------------------------------------
// Basic round-trip: SSE fixture replayed through the full provider stack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_basic_roundtrip() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/anthropic_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    // Collect all text tokens into one string
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "Hello world", "expected concatenated tokens to be 'Hello world', got: {text:?}");

    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done));
    assert!(has_done, "expected a Done event in the stream");
}

// ---------------------------------------------------------------------------
// Token sequence: order must be Token…, Usage, Done
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_event_ordering() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/anthropic_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    // Done must be the last event
    assert!(
        matches!(events.last(), Some(ProviderEvent::Done)),
        "last event must be Done, got: {:?}",
        events.last()
    );

    // At least one Usage event must be present
    let usage_count = events.iter().filter(|e| matches!(e, ProviderEvent::Usage(_))).count();
    assert!(usage_count >= 1, "expected at least one Usage event");

    // No events after Done
    let done_pos = events.iter().position(|e| matches!(e, ProviderEvent::Done)).unwrap();
    assert_eq!(done_pos, events.len() - 1, "Done must be the very last event");
}

// ---------------------------------------------------------------------------
// Usage values from the fixture: input=10, output=2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_usage_values() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/anthropic_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    // Sum all Usage events to get total tokens
    let (total_input, total_output) = events.iter().fold((0u32, 0u32), |(i, o), e| match e {
        ProviderEvent::Usage(u) => (i + u.input, o + u.output),
        _ => (i, o),
    });

    assert_eq!(total_input, 10, "expected 10 input tokens from fixture");
    assert_eq!(total_output, 2, "expected 2 output tokens from fixture");
}

// ---------------------------------------------------------------------------
// HTTP 401 is surfaced as ProviderError::Http
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_http_error_401() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("bad-key", server.uri());
    let result = provider.complete(make_request()).await;

    assert!(result.is_err(), "expected an error for 401 response");
    let err = result.err().unwrap();
    let err_str = err.to_string();
    assert!(
        err_str.contains("401") || err_str.to_lowercase().contains("http"),
        "expected error message to reference the HTTP status, got: {err_str}"
    );
}

// ---------------------------------------------------------------------------
// HTTP 429 is surfaced as ProviderError::RateLimit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_rate_limit_429() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limit exceeded"}}"#),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let result = provider.complete(make_request()).await;

    assert!(result.is_err(), "expected an error for 429 response");
    let err = result.err().unwrap();
    let err_str = err.to_string();
    assert!(
        err_str.to_lowercase().contains("rate") || err_str.contains("429"),
        "expected rate-limit error, got: {err_str}"
    );
}

// ---------------------------------------------------------------------------
// Prompt caching — cache_read_input_tokens is surfaced in TokenUsage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_anthropic_parses_cache_read_tokens() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/anthropic_cached.sse");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    // Sum cache_read across all Usage events.
    let total_cache_read: u32 = events.iter().fold(0u32, |acc, e| match e {
        ProviderEvent::Usage(u) => acc + u.cache_read,
        _ => acc,
    });

    assert_eq!(
        total_cache_read, 3000,
        "expected cache_read=3000 from the cached fixture, got {total_cache_read}"
    );

    // The non-cached portion is billed as regular input tokens.
    let total_input: u32 = events.iter().fold(0u32, |acc, e| match e {
        ProviderEvent::Usage(u) => acc + u.input,
        _ => acc,
    });

    assert_eq!(
        total_input, 50,
        "expected input_tokens=50 from the cached fixture, got {total_input}"
    );

    // Done must be present.
    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::Done)),
        "expected a Done event in the stream"
    );
}
