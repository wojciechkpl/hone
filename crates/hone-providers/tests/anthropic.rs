use std::collections::HashMap;

use hone_proto::{Message, ModelId, ProviderEvent, ToolSchema};
use hone_providers::adapters::anthropic::{build_request_body, extract_sse_events, parse_sse_data};
use hone_providers::traits::CompletionRequest;

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn make_request(messages: Vec<Message>) -> CompletionRequest {
    CompletionRequest {
        model: ModelId::new("anthropic:claude-sonnet-4-5"),
        messages,
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
    }
}

#[test]
fn test_build_request_body() {
    let request = CompletionRequest {
        model: ModelId::new("anthropic:claude-sonnet-4-5"),
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
    };

    let body = build_request_body(&request);

    // Model name must be the portion after the colon
    assert_eq!(body["model"].as_str().unwrap(), "claude-sonnet-4-5");

    // max_tokens must be present
    assert!(body["max_tokens"].as_u64().is_some());

    // stream must be true
    assert_eq!(body["stream"].as_bool(), Some(true));

    // messages array must be present with one entry
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str().unwrap(), "user");

    // tools array must be present with one entry
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"].as_str().unwrap(), "read_file");
    assert!(tools[0]["input_schema"].is_object());
}

#[test]
fn test_extract_system_message() {
    let request = make_request(vec![
        Message::system("You are a helpful assistant."),
        Message::user("Hi"),
    ]);

    let body = build_request_body(&request);

    // System must be extracted to the top-level field as an array of text blocks
    // (required for Anthropic prompt caching via cache_control breakpoints).
    let system_arr = body["system"].as_array().expect("system must be an array");
    assert!(!system_arr.is_empty());
    assert_eq!(
        system_arr[0]["text"].as_str().unwrap(),
        "You are a helpful assistant."
    );

    // messages array must NOT contain the system message
    let messages = body["messages"].as_array().unwrap();
    for msg in messages {
        assert_ne!(
            msg["role"].as_str().unwrap_or(""),
            "system",
            "system role must not appear in the messages array"
        );
    }

    // Only the user message should remain
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str().unwrap(), "user");
}

#[test]
fn test_request_without_tools_omits_tools_field() {
    let request = make_request(vec![Message::user("hello")]);
    let body = build_request_body(&request);
    assert!(body["tools"].is_null(), "tools field should be absent when empty");
}

// ---------------------------------------------------------------------------
// SSE parsing — text deltas
// ---------------------------------------------------------------------------

#[test]
fn test_parse_text_delta_event() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello, world!"}}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_data(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ProviderEvent::Token { text: "Hello, world!".to_owned() }
    );
}

#[test]
fn test_parse_empty_text_delta_produces_no_event() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":""}}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_data(data, &mut tool_states);
    assert!(events.is_empty());
}

// ---------------------------------------------------------------------------
// SSE parsing — tool use
// ---------------------------------------------------------------------------

#[test]
fn test_parse_tool_use_events() {
    let mut tool_states = HashMap::new();

    // 1. content_block_start announces a tool_use block at index 0
    let start = r#"{
        "type": "content_block_start",
        "index": 0,
        "content_block": { "type": "tool_use", "id": "toolu_abc123", "name": "read_file", "input": {} }
    }"#;

    let events = parse_sse_data(start, &mut tool_states);
    assert!(events.is_empty(), "no event should be emitted on block_start");
    assert!(tool_states.contains_key(&0), "tool state should be tracked at index 0");

    // 2. input_json_delta — first chunk
    let delta1 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
    let events = parse_sse_data(delta1, &mut tool_states);
    assert!(events.is_empty());

    // 3. input_json_delta — second chunk
    let delta2 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"/tmp/file.txt\"}"}}"#;
    let events = parse_sse_data(delta2, &mut tool_states);
    assert!(events.is_empty());

    // 4. content_block_stop — the ToolCall event must now be emitted
    let stop = r#"{"type":"content_block_stop","index":0}"#;
    let events = parse_sse_data(stop, &mut tool_states);

    assert_eq!(events.len(), 1, "expected exactly one ToolCall event");
    match &events[0] {
        ProviderEvent::ToolCall(req) => {
            assert_eq!(req.id, "toolu_abc123");
            assert_eq!(req.name, "read_file");
            assert_eq!(req.arguments["path"], "/tmp/file.txt");
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    // Tool state should be cleaned up after stop
    assert!(!tool_states.contains_key(&0));
}

#[test]
fn test_parse_tool_use_with_empty_input() {
    let mut tool_states = HashMap::new();

    let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_xyz","name":"list_files","input":{}}}"#;
    parse_sse_data(start, &mut tool_states);

    // No input_json_delta events — tool takes no arguments
    let stop = r#"{"type":"content_block_stop","index":0}"#;
    let events = parse_sse_data(stop, &mut tool_states);

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderEvent::ToolCall(req) => {
            assert_eq!(req.name, "list_files");
            // Empty input should parse to an empty object
            assert!(req.arguments.is_object());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SSE parsing — message_stop → Done
// ---------------------------------------------------------------------------

#[test]
fn test_parse_message_stop() {
    let data = r#"{"type":"message_stop"}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_data(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0], ProviderEvent::Done);
}

// ---------------------------------------------------------------------------
// SSE parsing — usage events
// ---------------------------------------------------------------------------

#[test]
fn test_parse_usage_event() {
    // message_delta carries output token count
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_data(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderEvent::Usage(usage) => {
            assert_eq!(usage.output, 42);
        }
        other => panic!("expected Usage event, got {other:?}"),
    }
}

#[test]
fn test_parse_message_start_usage() {
    // message_start carries input token count
    let data = r#"{"type":"message_start","message":{"id":"msg_001","model":"claude-sonnet-4-5","usage":{"input_tokens":100,"output_tokens":0}}}"#;
    let mut tool_states = HashMap::new();

    let events = parse_sse_data(data, &mut tool_states);

    assert_eq!(events.len(), 1);
    match &events[0] {
        ProviderEvent::Usage(usage) => {
            assert_eq!(usage.input, 100);
            assert_eq!(usage.output, 0);
        }
        other => panic!("expected Usage event from message_start, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SSE chunk extraction
// ---------------------------------------------------------------------------

#[test]
fn test_extract_sse_events_parses_chunk() {
    let chunk = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\n";
    let pairs = extract_sse_events(chunk);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, "content_block_delta");
    assert_eq!(pairs[0].1, "{\"type\":\"content_block_delta\"}");
}

#[test]
fn test_extract_sse_events_multiple_blocks() {
    let chunk = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\"}\n\n",
    );
    let pairs = extract_sse_events(chunk);
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "message_start");
    assert_eq!(pairs[1].0, "content_block_start");
}

#[test]
fn test_extract_sse_events_ignores_empty_data() {
    let chunk = "event: ping\n\n";
    let pairs = extract_sse_events(chunk);
    assert!(pairs.is_empty());
}

// ---------------------------------------------------------------------------
// Prompt caching — cache_control breakpoints
// ---------------------------------------------------------------------------

#[test]
fn test_request_has_cache_control_on_system() {
    let request = make_request(vec![
        Message::system("long system prompt with lots of content"),
        Message::user("hi"),
    ]);

    let body = build_request_body(&request);

    // System must be an array so that cache_control can be attached.
    let system = body.get("system").expect("system field must be present");
    assert!(system.is_array(), "system must be an array for cache_control support");

    let arr = system.as_array().unwrap();
    assert!(!arr.is_empty());

    let last = arr.last().unwrap();
    assert!(
        last.get("cache_control").is_some(),
        "last system block must have cache_control"
    );
    assert_eq!(
        last["cache_control"]["type"].as_str(),
        Some("ephemeral"),
        "cache_control type must be ephemeral"
    );
}

#[test]
fn test_request_has_cache_control_on_tools_when_present() {
    let request = CompletionRequest {
        model: ModelId::new("anthropic:claude-sonnet-4-5"),
        messages: vec![Message::user("hi")],
        tools: vec![
            ToolSchema {
                name: "read_file".to_owned(),
                description: "read a file".to_owned(),
                parameters: serde_json::json!({"type": "object"}),
            },
            ToolSchema {
                name: "write_file".to_owned(),
                description: "write a file".to_owned(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ],
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
    };

    let body = build_request_body(&request);
    let tools = body.get("tools").expect("tools field must be present").as_array().unwrap();
    assert_eq!(tools.len(), 2);

    // First tool must NOT have cache_control (only the last one gets the breakpoint).
    assert!(
        tools[0].get("cache_control").is_none(),
        "first tool must not have cache_control"
    );

    // Last tool must have cache_control.
    assert!(
        tools[1].get("cache_control").is_some(),
        "last tool must have cache_control"
    );
    assert_eq!(
        tools[1]["cache_control"]["type"].as_str(),
        Some("ephemeral"),
        "last tool cache_control type must be ephemeral"
    );
}

#[test]
fn test_request_without_tools_still_valid() {
    let request = make_request(vec![Message::user("hi")]);
    let body = build_request_body(&request);

    // tools field must be absent when no tools are provided.
    assert!(
        body.get("tools").is_none() || body["tools"].as_array().map_or(true, |a| a.is_empty()),
        "tools field must be absent or empty when no tools are provided"
    );

    // messages must be present.
    assert!(body.get("messages").is_some(), "messages must always be present");
}

#[test]
fn test_single_tool_gets_cache_control() {
    let request = CompletionRequest {
        model: ModelId::new("anthropic:claude-sonnet-4-5"),
        messages: vec![Message::user("hi")],
        tools: vec![ToolSchema {
            name: "list_files".to_owned(),
            description: "list files".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
        }],
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
    };

    let body = build_request_body(&request);
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);

    // A single tool is also the last tool, so it must have cache_control.
    assert!(
        tools[0].get("cache_control").is_some(),
        "the only tool must have cache_control when it is also the last"
    );
}
