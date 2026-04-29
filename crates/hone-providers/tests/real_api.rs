#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Integration tests against real APIs.
//!
//! These tests are marked `#[ignore]` so they do not run in the default
//! `cargo test`. To run them, set the relevant API keys and use:
//!
//! ```text
//! ANTHROPIC_API_KEY=sk-ant-... cargo test --test real_api -- --ignored
//! ```
//!
//! All three providers can be exercised at once:
//!
//! ```text
//! ANTHROPIC_API_KEY=sk-ant-... OPENAI_API_KEY=sk-... \
//!     cargo test -p hone-providers --test real_api -- --ignored --nocapture
//! ```

use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent, ToolSchema};
use hone_providers::{
    AnthropicProvider, CompletionRequest, ModelProvider, OllamaProvider, OpenAiProvider,
};

// ---------------------------------------------------------------------------
// Shared helper — works for any provider
// ---------------------------------------------------------------------------

async fn run_basic_test<P: ModelProvider>(provider: P, model_id: &str) {
    let request = CompletionRequest::new(
        ModelId::new(model_id),
        vec![Message::user(
            "Respond with the single word OK and nothing else.",
        )],
    );

    let stream = provider
        .complete(request)
        .await
        .expect("provider call failed");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_token = events
        .iter()
        .any(|e| matches!(e, ProviderEvent::Token { .. }));
    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done));

    assert!(has_token, "expected at least one Token event");
    assert!(has_done, "expected a Done event");
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_anthropic_live() {
    let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("ANTHROPIC_API_KEY not set, skipping");
        return;
    };

    let provider = AnthropicProvider::new(key);
    run_basic_test(provider, "anthropic:claude-sonnet-4-5").await;
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_openai_live() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("OPENAI_API_KEY not set, skipping");
        return;
    };

    let provider = OpenAiProvider::new(key);
    run_basic_test(provider, "openai:gpt-4o-mini").await;
}

// ---------------------------------------------------------------------------
// Ollama (local)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_ollama_live() {
    // Check if Ollama is reachable before attempting the test
    let client = reqwest::Client::new();
    let health = client
        .get("http://localhost:11434/api/tags")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if health.is_err() {
        eprintln!("Ollama not running on localhost:11434, skipping");
        return;
    }

    let provider = OllamaProvider::new("http://localhost:11434");
    run_basic_test(provider, "ollama:llama3.2").await;
}

// ---------------------------------------------------------------------------
// Error message quality — intentional 401
// ---------------------------------------------------------------------------

/// Verify that a 401 from Anthropic surfaces a human-readable message rather
/// than raw JSON. Does not require a real API key; uses a hardcoded fake one.
#[tokio::test]
#[ignore]
async fn test_anthropic_invalid_key_error_is_actionable() {
    let provider = AnthropicProvider::new("sk-ant-invalid-key-for-testing");
    let request = CompletionRequest::new(
        ModelId::new("anthropic:claude-haiku-4-5-20251001"),
        vec![Message::user("hi")],
    );

    let result = provider.complete(request).await;
    let err = match result {
        Ok(_) => panic!("expected auth error but got Ok"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid") || msg.contains("API key") || msg.contains("401"),
        "error message should mention invalid key or 401, got: {msg}"
    );
    assert!(
        !msg.contains('{'),
        "error message should not contain raw JSON, got: {msg}"
    );
}

/// Same check for OpenAI.
#[tokio::test]
#[ignore]
async fn test_openai_invalid_key_error_is_actionable() {
    let provider = OpenAiProvider::new("sk-invalid-key-for-testing");
    let request = CompletionRequest::new(
        ModelId::new("openai:gpt-4o-mini"),
        vec![Message::user("hi")],
    );

    let result = provider.complete(request).await;
    let err = match result {
        Ok(_) => panic!("expected auth error but got Ok"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("invalid") || msg.contains("API key") || msg.contains("401"),
        "error message should mention invalid key or 401, got: {msg}"
    );
    assert!(
        !msg.contains('{'),
        "error message should not contain raw JSON, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Tool-call happy path
// ---------------------------------------------------------------------------

/// Verify that Anthropic correctly emits a `ToolCall` event when a tool is
/// registered and the model decides to use it.
#[tokio::test]
#[ignore]
async fn test_anthropic_live_with_tool_call() {
    let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("ANTHROPIC_API_KEY not set, skipping");
        return;
    };

    let provider = AnthropicProvider::new(key);
    let mut request = CompletionRequest::new(
        ModelId::new("anthropic:claude-haiku-4-5-20251001"),
        vec![Message::user("What is 7 plus 3? Use the add tool.")],
    );
    request.tools = vec![ToolSchema {
        name: "add".to_string(),
        description: "Add two integers and return the sum.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        }),
    }];

    let stream = provider
        .complete(request)
        .await
        .expect("provider call failed");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_tool_call = events
        .iter()
        .any(|e| matches!(e, ProviderEvent::ToolCall(_)));
    assert!(
        has_tool_call,
        "expected at least one ToolCall event; got {events:?}"
    );
}

/// Same tool-call test for OpenAI.
#[tokio::test]
#[ignore]
async fn test_openai_live_with_tool_call() {
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("OPENAI_API_KEY not set, skipping");
        return;
    };

    let provider = OpenAiProvider::new(key);
    let mut request = CompletionRequest::new(
        ModelId::new("openai:gpt-4o-mini"),
        vec![Message::user("What is 7 plus 3? Use the add tool.")],
    );
    request.tools = vec![ToolSchema {
        name: "add".to_string(),
        description: "Add two integers and return the sum.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        }),
    }];

    let stream = provider
        .complete(request)
        .await
        .expect("provider call failed");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_tool_call = events
        .iter()
        .any(|e| matches!(e, ProviderEvent::ToolCall(_)));
    assert!(
        has_tool_call,
        "expected at least one ToolCall event; got {events:?}"
    );
}
