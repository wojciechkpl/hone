use hone_proto::*;

#[test]
fn message_roundtrip_user() {
    let msg = Message::user("hello world");
    let json = serde_json::to_string(&msg).unwrap();
    let decoded: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn message_roundtrip_assistant_with_tool_calls() {
    let msg = Message {
        role: Role::Assistant,
        content: Content::Text("thinking...".into()),
        tool_calls: vec![ToolCallRequest {
            id: "tc_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/foo/bar.rs"}),
        }],
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let decoded: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn message_roundtrip_tool_result() {
    let msg = Message::tool_result("tc_1".into(), "file contents here");
    let json = serde_json::to_string(&msg).unwrap();
    let decoded: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, decoded);
    assert_eq!(msg.tool_call_id.as_deref(), Some("tc_1"));
}

#[test]
fn message_roundtrip_multipart_content() {
    let content = Content::Parts(vec![
        ContentPart::Text {
            text: "look at this".into(),
        },
        ContentPart::Image {
            media_type: "image/png".into(),
            data: "base64data".into(),
        },
    ]);
    let msg = Message {
        role: Role::User,
        content,
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let decoded: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn content_as_text_plain() {
    let c = Content::Text("hello".into());
    assert_eq!(c.as_text(), "hello");
}

#[test]
fn content_as_text_multipart() {
    let c = Content::Parts(vec![
        ContentPart::Text {
            text: "hello ".into(),
        },
        ContentPart::Image {
            media_type: "image/png".into(),
            data: "...".into(),
        },
        ContentPart::Text {
            text: "world".into(),
        },
    ]);
    assert_eq!(c.as_text(), "hello world");
}

#[test]
fn model_id_display() {
    let id = ModelId::from("anthropic:claude-opus-4-5");
    assert_eq!(id.to_string(), "anthropic:claude-opus-4-5");
}

#[test]
fn model_id_from_str() {
    let id: ModelId = "openai:gpt-4o".parse().unwrap();
    assert_eq!(id.as_str(), "openai:gpt-4o");
}

#[test]
fn model_id_provider_and_name() {
    let id = ModelId::from("anthropic:claude-opus-4-5");
    assert_eq!(id.provider(), Some("anthropic"));
    assert_eq!(id.model_name(), "claude-opus-4-5");
}

#[test]
fn model_id_no_provider_prefix() {
    let id = ModelId::from("qwen3:32b");
    assert_eq!(id.provider(), Some("qwen3"));
    assert_eq!(id.model_name(), "32b");
}

#[test]
fn model_id_no_colon() {
    let id = ModelId::from("local-model");
    assert_eq!(id.provider(), None);
    assert_eq!(id.model_name(), "local-model");
}

#[test]
fn token_usage_add() {
    let a = TokenUsage {
        input: 100,
        output: 50,
        cache_read: 10,
    };
    let b = TokenUsage {
        input: 200,
        output: 75,
        cache_read: 20,
    };
    let sum = a + b;
    assert_eq!(
        sum,
        TokenUsage {
            input: 300,
            output: 125,
            cache_read: 30,
        }
    );
}

#[test]
fn token_usage_total() {
    let u = TokenUsage {
        input: 100,
        output: 50,
        cache_read: 10,
    };
    assert_eq!(u.total(), 160);
}

#[test]
fn token_usage_default_is_zero() {
    let u = TokenUsage::default();
    assert_eq!(u.total(), 0);
}

#[test]
fn token_usage_add_is_associative() {
    let a = TokenUsage {
        input: 10,
        output: 20,
        cache_read: 5,
    };
    let b = TokenUsage {
        input: 30,
        output: 40,
        cache_read: 15,
    };
    let c = TokenUsage {
        input: 50,
        output: 60,
        cache_read: 25,
    };
    assert_eq!((a + b) + c, a + (b + c));
}

#[test]
fn session_id_is_uuid_v7() {
    let id = SessionId::new();
    let uuid = id.as_uuid();
    assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
}

#[test]
fn session_id_display_is_uuid_format() {
    let id = SessionId::new();
    let s = id.to_string();
    // UUID format: 8-4-4-4-12
    assert_eq!(s.len(), 36);
    assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
}

#[test]
fn turn_id_is_uuid_v7() {
    let id = TurnId::new();
    let uuid = id.as_uuid();
    assert_eq!(uuid.get_version(), Some(uuid::Version::SortRand));
}

#[test]
fn session_id_roundtrip() {
    let id = SessionId::new();
    let json = serde_json::to_string(&id).unwrap();
    let decoded: SessionId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, decoded);
}

#[test]
fn tool_call_request_roundtrip() {
    let req = ToolCallRequest {
        id: "tc_42".into(),
        name: "write_file".into(),
        arguments: serde_json::json!({
            "path": "/tmp/test.txt",
            "content": "hello"
        }),
    };
    let json = serde_json::to_string(&req).unwrap();
    let decoded: ToolCallRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(req, decoded);
}

#[test]
fn tool_schema_roundtrip() {
    let schema = ToolSchema {
        name: "read_file".into(),
        description: "Read a file from disk".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }),
    };
    let json = serde_json::to_string(&schema).unwrap();
    let decoded: ToolSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(schema, decoded);
}

#[test]
fn tool_result_error_flag() {
    let result = ToolResult {
        tool_call_id: "tc_1".into(),
        content: serde_json::json!({"error": "file not found"}),
        is_error: true,
    };
    assert!(result.is_error);
    let json = serde_json::to_string(&result).unwrap();
    let decoded: ToolResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result, decoded);
}

#[test]
fn tool_layer_serialization() {
    let native = ToolLayer::Native;
    let mcp = ToolLayer::Mcp;
    assert_eq!(
        serde_json::to_string(&native).unwrap(),
        "\"Native\""
    );
    assert_eq!(
        serde_json::to_string(&mcp).unwrap(),
        "\"Mcp\""
    );
}

#[test]
fn provider_event_token_roundtrip() {
    let event = ProviderEvent::Token {
        text: "Hello".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let decoded: ProviderEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, decoded);
}

#[test]
fn provider_event_done_roundtrip() {
    let event = ProviderEvent::Done;
    let json = serde_json::to_string(&event).unwrap();
    let decoded: ProviderEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, decoded);
}

#[test]
fn agent_event_variants_roundtrip() {
    let events = vec![
        AgentEvent::Token {
            text: "hi".into(),
        },
        AgentEvent::ToolCallStart {
            id: "tc_1".into(),
            name: "read_file".into(),
        },
        AgentEvent::ToolCallEnd {
            id: "tc_1".into(),
            result: serde_json::json!("contents"),
            is_error: false,
        },
        AgentEvent::Done {
            usage: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 0,
            },
        },
        AgentEvent::Error {
            message: "boom".into(),
        },
        AgentEvent::ApprovalRequired {
            action: "write_file".into(),
            description: "Write to /etc/passwd".into(),
        },
    ];
    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }
}

#[test]
fn pricing_entry_calculate_cost() {
    let entry = PricingEntry {
        model: ModelId::from("anthropic:claude-opus-4-5"),
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: Some(1.5),
    };
    // 1000 input tokens = $0.015
    // 500 output tokens = $0.0375
    // 200 cache tokens = $0.0003
    let cost = entry.calculate_cost(1000, 500, 200);
    let expected = 0.015 + 0.0375 + 0.0003;
    assert!((cost - expected).abs() < 1e-10);
}

#[test]
fn pricing_entry_no_cache_rate() {
    let entry = PricingEntry {
        model: ModelId::from("ollama:qwen3"),
        input_per_million: 0.0,
        output_per_million: 0.0,
        cache_read_per_million: None,
    };
    let cost = entry.calculate_cost(10000, 5000, 1000);
    assert_eq!(cost, 0.0);
}

#[test]
fn role_serialization() {
    assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(
        serde_json::to_string(&Role::Assistant).unwrap(),
        "\"assistant\""
    );
    assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
}

#[test]
fn model_info_roundtrip() {
    let info = ModelInfo {
        id: ModelId::from("anthropic:claude-opus-4-5"),
        display_name: "Claude Opus 4.5".into(),
        context_length: 200_000,
        supports_tools: true,
        supports_streaming: true,
    };
    let json = serde_json::to_string(&info).unwrap();
    let decoded: ModelInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn provider_health_variants() {
    let healthy = ProviderHealth::Healthy;
    let degraded = ProviderHealth::Degraded {
        reason: "slow".into(),
    };
    let unavailable = ProviderHealth::Unavailable {
        reason: "down".into(),
    };

    for variant in [healthy, degraded, unavailable] {
        let json = serde_json::to_string(&variant).unwrap();
        let decoded: ProviderHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, decoded);
    }
}
