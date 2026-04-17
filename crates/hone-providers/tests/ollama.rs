use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::adapters::ollama::{build_request_body, health_url, parse_ndjson_line};
use hone_providers::traits::CompletionRequest;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: ModelId::new("ollama:llama3.2"),
        messages,
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

#[test]
fn test_ollama_build_request_body() {
    let request = make_request(vec![
        Message::system("You are a helpful assistant."),
        Message::user("Hello"),
    ]);

    let body = build_request_body(&request);

    // Model name must be the portion after the colon
    assert_eq!(body["model"].as_str().unwrap(), "llama3.2");

    // stream must be true
    assert_eq!(body["stream"].as_bool(), Some(true));

    // messages array must contain both messages
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");

    // No tools field — Ollama does not support tools
    assert!(body["tools"].is_null());
}

#[test]
fn test_ollama_build_request_body_includes_tools_when_provided() {
    // Ollama now supports tool calling (structured for models like Llama 3.1+,
    // text-based fallback for Qwen-style models). When tools are provided, the
    // adapter must emit them in the request body.
    let request = CompletionRequest {
        model: ModelId::new("ollama:llama3.2"),
        messages: vec![Message::user("hi")],
        tools: vec![hone_proto::ToolSchema {
            name: "read_file".to_owned(),
            description: "Read a file".to_owned(),
            parameters: serde_json::json!({}),
        }],
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
    };

    let body = build_request_body(&request);
    assert!(
        !body["tools"].is_null(),
        "Ollama adapter must emit tools when the request contains tools"
    );
    assert_eq!(body["tools"][0]["function"]["name"], "read_file");
}

// ---------------------------------------------------------------------------
// NDJSON parsing — token chunks
// ---------------------------------------------------------------------------

#[test]
fn test_ollama_parse_token() {
    let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":"Hello"},"done":false}"#;
    let events = parse_ndjson_line(line);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ProviderEvent::Token { text: "Hello".to_owned() }
    );
}

#[test]
fn test_ollama_parse_empty_content_produces_no_event() {
    let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":false}"#;
    let events = parse_ndjson_line(line);
    assert!(events.is_empty());
}

// ---------------------------------------------------------------------------
// NDJSON parsing — done chunk
// ---------------------------------------------------------------------------

#[test]
fn test_ollama_parse_done() {
    let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"total_duration":1234567,"eval_count":15,"prompt_eval_count":10}"#;
    let events = parse_ndjson_line(line);

    // Expect Usage followed by Done
    assert_eq!(events.len(), 2);

    match &events[0] {
        ProviderEvent::Usage(usage) => {
            assert_eq!(usage.input, 10, "prompt_eval_count maps to input tokens");
            assert_eq!(usage.output, 15, "eval_count maps to output tokens");
            assert_eq!(usage.cache_read, 0);
        }
        other => panic!("expected Usage, got {other:?}"),
    }

    assert_eq!(events[1], ProviderEvent::Done);
}

#[test]
fn test_ollama_parse_done_without_eval_counts() {
    // Some Ollama responses omit eval counts — should still emit Done without Usage
    let line = r#"{"model":"llama3.2","message":{"role":"assistant","content":""},"done":true}"#;
    let events = parse_ndjson_line(line);

    // Without token counts there is no Usage event, only Done
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], ProviderEvent::Done);
}

// ---------------------------------------------------------------------------
// Health check URL
// ---------------------------------------------------------------------------

#[test]
fn test_ollama_health_url() {
    let url = health_url("http://localhost:11434");
    assert_eq!(url, "http://localhost:11434/api/tags");
}

#[test]
fn test_ollama_health_url_custom_host() {
    let url = health_url("http://myserver:8080");
    assert_eq!(url, "http://myserver:8080/api/tags");
}
