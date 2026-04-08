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
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{AnthropicProvider, CompletionRequest, ModelProvider, OllamaProvider, OpenAiProvider};

// ---------------------------------------------------------------------------
// Shared helper — works for any provider
// ---------------------------------------------------------------------------

async fn run_basic_test<P: ModelProvider>(provider: P, model_id: &str) {
    let request = CompletionRequest::new(
        ModelId::new(model_id),
        vec![Message::user("Respond with the single word OK and nothing else.")],
    );

    let stream = provider.complete(request).await.expect("provider call failed");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_token = events.iter().any(|e| matches!(e, ProviderEvent::Token { .. }));
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
