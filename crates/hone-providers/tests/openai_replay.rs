use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{CompletionRequest, ModelProvider, OpenAiProvider};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request() -> CompletionRequest {
    CompletionRequest::new(
        ModelId::new("openai:gpt-4o-mini"),
        vec![Message::user("hello")],
    )
}

// ---------------------------------------------------------------------------
// Basic round-trip: SSE fixture replayed through the full provider stack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_basic_roundtrip() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/openai_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(text, "Hello world", "expected 'Hello world', got: {text:?}");

    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done));
    assert!(has_done, "expected a Done event");
}

// ---------------------------------------------------------------------------
// Token sequence: Done must be the last event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_event_ordering() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/openai_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    assert!(
        matches!(events.last(), Some(ProviderEvent::Done)),
        "last event must be Done, got: {:?}",
        events.last()
    );
}

// ---------------------------------------------------------------------------
// Usage values from the fixture: input=10, output=2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_usage_values() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/openai_basic.sse");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

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
async fn test_openai_http_error_401() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"error":{"message":"Invalid API key","type":"invalid_request_error","code":"invalid_api_key"}}"#),
        )
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url("bad-key", server.uri());
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
async fn test_openai_rate_limit_429() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"error":{"message":"Rate limit exceeded","type":"rate_limit_error","code":"rate_limit_exceeded"}}"#),
        )
        .mount(&server)
        .await;

    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let result = provider.complete(make_request()).await;

    assert!(result.is_err(), "expected an error for 429 response");
    let err = result.err().unwrap();
    let err_str = err.to_string();
    assert!(
        err_str.to_lowercase().contains("rate") || err_str.contains("429"),
        "expected rate-limit error, got: {err_str}"
    );
}
