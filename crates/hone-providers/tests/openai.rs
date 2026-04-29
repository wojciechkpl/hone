#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::HashMap;

use hone_proto::{Message, ModelId, ProviderEvent, ToolSchema};
use hone_providers::adapters::openai::{build_request_body, parse_sse_chunk};
use hone_providers::traits::CompletionRequest;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: ModelId::new("openai:gpt-4o"),
        messages,
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
        cancel: None,
        tool_results: None,
    }
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

#[test]
fn test_openai_build_request_body() {
    let request = CompletionRequest {
        model: ModelId::new("openai:gpt-4o"),
        messages: vec![Message::user("Hello")],
        tools: vec![ToolSchema {
            name: "read_file".to_owned(),
            description: "Read a file from disk".to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }],
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
        cancel: None,
        tool_results: None,
    };

    let body = build_request_body(&request);

    // Model name must be the portion after the colon
    assert_eq!(body["model"].as_str().unwrap(), "gpt-4o");

    // stream must be true
    assert_eq!(body["stream"].as_bool(), Some(true));

    // messages array must be present with one user entry
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str().unwrap(), "user");

    // tools array must be present with one entry in OpenAI format
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"].as_str().unwrap(), "function");
    assert_eq!(tools[0]["function"]["name"].as_str().unwrap(), "read_file");
    assert!(tools[0]["function"]["parameters"].is_object());
}

#[test]
fn test_openai_system_message_as_role() {
    // Unlike Anthropic, OpenAI keeps system messages in the messages array.
    let request = make_request(vec![
        Message::system("You are a helpful assistant."),
        Message::user("Hi"),
    ]);

    let body = build_request_body(&request);

    // There must be NO top-level "system" field
    assert!(
        body["system"].is_null(),
        "OpenAI does not use a top-level system field"
    );

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);

    // First message must have role "system"
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(
        messages[0]["content"].as_str().unwrap(),
        "You are a helpful assistant."
    );

    // Second message must be the user message
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");
}

#[test]
fn test_openai_build_request_without_tools_omits_tools_field() {
    let request = make_request(vec![Message::user("hello")]);
    let body = build_request_body(&request);
    assert!(
        body["tools"].is_null(),
        "tools field should be absent when empty"
    );
}

// ---------------------------------------------------------------------------
// SSE parsing — text delta
// ---------------------------------------------------------------------------

#[test]
fn test_openai_parse_text_delta() {
    let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_chunk(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ProviderEvent::Token {
            text: "Hello".to_owned()
        }
    );
}

#[test]
fn test_openai_parse_empty_content_produces_no_event() {
    let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":""},"finish_reason":null}]}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_chunk(data, &mut tool_states);
    assert!(events.is_empty());
}

// ---------------------------------------------------------------------------
// SSE parsing — tool call accumulation
// ---------------------------------------------------------------------------

#[test]
fn test_openai_parse_tool_call() {
    let mut tool_states = HashMap::new();

    // First chunk: announces tool call with id and function name
    let chunk1 = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#;
    let events = parse_sse_chunk(chunk1, &mut tool_states);
    assert!(events.is_empty(), "no event yet — still accumulating");

    // Second chunk: partial arguments
    let chunk2 = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;
    let events = parse_sse_chunk(chunk2, &mut tool_states);
    assert!(events.is_empty());

    // Third chunk: remaining arguments
    let chunk3 = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"/tmp/file.txt\"}"}}]},"finish_reason":null}]}"#;
    let events = parse_sse_chunk(chunk3, &mut tool_states);
    assert!(events.is_empty());

    // Final chunk: finish_reason "tool_calls" triggers emission
    let chunk4 =
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#;
    let events = parse_sse_chunk(chunk4, &mut tool_states);

    // Expect ToolCall + Done
    assert_eq!(events.len(), 2);
    match &events[0] {
        ProviderEvent::ToolCall(req) => {
            assert_eq!(req.id, "call_abc123");
            assert_eq!(req.name, "read_file");
            assert_eq!(req.arguments["path"], "/tmp/file.txt");
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
    assert_eq!(events[1], ProviderEvent::Done);
}

// ---------------------------------------------------------------------------
// SSE parsing — finish_reason: stop → Done
// ---------------------------------------------------------------------------

#[test]
fn test_openai_parse_done() {
    let data = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_chunk(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0], ProviderEvent::Done);
}

#[test]
fn finish_reason_length_emits_truncated() {
    let mut tool_states = HashMap::new();

    // Start streaming a tool call but never complete it before max_tokens.
    let chunk1 = r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"grep","arguments":"{\"pattern\":"}}]},"finish_reason":null}]}"#;
    let evts = parse_sse_chunk(chunk1, &mut tool_states);
    assert!(evts.is_empty());

    let chunk_last =
        r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#;
    let evts = parse_sse_chunk(chunk_last, &mut tool_states);

    let lost = evts
        .iter()
        .find_map(|e| match e {
            ProviderEvent::Truncated { lost_tool_calls } => Some(lost_tool_calls),
            _ => None,
        })
        .expect("should emit Truncated on finish_reason=length");
    assert_eq!(lost, &vec!["grep".to_string()]);
    assert!(evts.contains(&ProviderEvent::Done));
    assert!(tool_states.is_empty());
}

#[test]
fn test_openai_parse_done_sentinel() {
    let mut tool_states = HashMap::new();
    let events = parse_sse_chunk("[DONE]", &mut tool_states);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0], ProviderEvent::Done);
}

// ---------------------------------------------------------------------------
// SSE parsing — usage in final chunk
// ---------------------------------------------------------------------------

#[test]
fn test_openai_parse_usage() {
    // Usage appears in the chunk that also has an empty choices array when
    // stream_options.include_usage is set
    let data = r#"{"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_chunk(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderEvent::Usage(usage) => {
            assert_eq!(usage.input, 10);
            assert_eq!(usage.output, 20);
        }
        other => panic!("expected Usage, got {other:?}"),
    }
}
