use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{CompletionRequest, ModelProvider, OllamaProvider};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request() -> CompletionRequest {
    CompletionRequest::new(
        ModelId::new("ollama:llama3.2"),
        vec![Message::user("hello")],
    )
}

// ---------------------------------------------------------------------------
// Basic round-trip: NDJSON fixture replayed through the full provider stack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_basic_roundtrip() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/ollama_basic.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    // OllamaProvider::new already takes the base URL directly — no separate
    // with_base_url needed.
    let provider = OllamaProvider::new(server.uri());
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
async fn test_ollama_event_ordering() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/ollama_basic.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    assert!(
        matches!(events.last(), Some(ProviderEvent::Done)),
        "last event must be Done, got: {:?}",
        events.last()
    );

    let done_pos = events
        .iter()
        .position(|e| matches!(e, ProviderEvent::Done))
        .unwrap();
    assert_eq!(
        done_pos,
        events.len() - 1,
        "Done must be the very last event"
    );
}

// ---------------------------------------------------------------------------
// Usage values from the fixture: prompt_eval_count=10, eval_count=2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_usage_values() {
    let server = MockServer::start().await;
    let body = include_str!("fixtures/ollama_basic.ndjson");

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(server.uri());
    let stream = provider.complete(make_request()).await.unwrap();
    let events: Vec<ProviderEvent> = stream.collect().await;

    let (total_input, total_output) = events.iter().fold((0u32, 0u32), |(i, o), e| match e {
        ProviderEvent::Usage(u) => (i + u.input, o + u.output),
        _ => (i, o),
    });

    assert_eq!(
        total_input, 10,
        "expected 10 input tokens (prompt_eval_count)"
    );
    assert_eq!(total_output, 2, "expected 2 output tokens (eval_count)");
}

// ---------------------------------------------------------------------------
// HTTP 500 is surfaced as ProviderError::Http
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_http_error_500() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(500)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"error":"model not found"}"#),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(server.uri());
    let result = provider.complete(make_request()).await;

    assert!(result.is_err(), "expected an error for 500 response");
    let err = result.err().unwrap();
    let err_str = err.to_string();
    assert!(
        err_str.contains("500") || err_str.to_lowercase().contains("http"),
        "expected error message to reference the HTTP status, got: {err_str}"
    );
}
