#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;

use futures::StreamExt;
use hone_proto::{ModelId, ModelInfo, ProviderEvent, ProviderHealth};
use hone_providers::acp::{AcpProvider, AcpTransport, MockAgentConfig, create_mock_agent_script};
// NOTE: create_mock_agent_script returns (PathBuf dir, PathBuf script).
// Keep `_dir` alive for the duration of each test to prevent premature cleanup.
use hone_providers::acp::types::{
    ClientInfo, InitializeParams, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    MessageContent, SessionNewParams, SessionNewResult, UpdatePayload,
};
use hone_providers::{CompletionRequest, ModelProvider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn simple_request() -> CompletionRequest {
    CompletionRequest::new(
        ModelId::new("acp:test-model"),
        vec![hone_proto::Message::user("Say hello")],
    )
}

// ---------------------------------------------------------------------------
// types.rs — serialization round-trips
// ---------------------------------------------------------------------------

#[test]
fn test_acp_types_jsonrpc_request_roundtrip() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: 42,
        method: "initialize".to_owned(),
        params: Some(serde_json::json!({"protocolVersion": 1})),
    };

    let json = serde_json::to_string(&req).expect("serialize");
    let back: JsonRpcRequest = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back.jsonrpc, "2.0");
    assert_eq!(back.id, 42);
    assert_eq!(back.method, "initialize");
    assert!(back.params.is_some());
}

#[test]
fn test_acp_types_jsonrpc_response_roundtrip() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".to_owned(),
        id: 1,
        result: Some(serde_json::json!({"sessionId": "sess_abc"})),
        error: None,
    };

    let json = serde_json::to_string(&resp).expect("serialize");
    let back: JsonRpcResponse = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back.id, 1);
    assert!(back.result.is_some());
    assert!(back.error.is_none());
}

#[test]
fn test_acp_types_notification_roundtrip() {
    let notif = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: "session/update".to_owned(),
        params: Some(serde_json::json!({"sessionId": "sess_abc"})),
    };

    let json = serde_json::to_string(&notif).expect("serialize");
    let back: JsonRpcNotification = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(back.method, "session/update");
}

#[test]
fn test_acp_update_payload_message_chunk_parse() {
    let json =
        r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello!"}}"#;
    let payload: UpdatePayload = serde_json::from_str(json).expect("deserialize");

    match payload {
        UpdatePayload::MessageChunk { content } => {
            assert_eq!(content.text, "Hello!");
            assert_eq!(content.content_type, "text");
        }
        other => panic!("expected MessageChunk, got {other:?}"),
    }
}

#[test]
fn test_acp_update_payload_tool_call_parse() {
    let json = r#"{"sessionUpdate":"tool_call","toolCallId":"call_001","title":"Reading file","kind":"read","status":"pending"}"#;
    let payload: UpdatePayload = serde_json::from_str(json).expect("deserialize");

    match payload {
        UpdatePayload::ToolCall {
            tool_call_id,
            title,
            kind,
            status,
            raw_input,
        } => {
            assert_eq!(tool_call_id, "call_001");
            assert_eq!(title, "Reading file");
            assert_eq!(kind, "read");
            assert_eq!(status, "pending");
            assert!(
                raw_input.is_none(),
                "expected absent rawInput to deserialize as None, got {raw_input:?}"
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn test_acp_update_payload_tool_call_update_parse() {
    let json =
        r#"{"sessionUpdate":"tool_call_update","toolCallId":"call_001","status":"completed"}"#;
    let payload: UpdatePayload = serde_json::from_str(json).expect("deserialize");

    match payload {
        UpdatePayload::ToolCallUpdate {
            tool_call_id,
            status,
        } => {
            assert_eq!(tool_call_id, "call_001");
            assert_eq!(status, "completed");
        }
        other => panic!("expected ToolCallUpdate, got {other:?}"),
    }
}

#[test]
fn test_acp_update_payload_message_chunk_serialize() {
    let payload = UpdatePayload::MessageChunk {
        content: MessageContent {
            content_type: "text".to_owned(),
            text: "chunk text".to_owned(),
        },
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    assert!(json.contains("agent_message_chunk"));
    assert!(json.contains("chunk text"));
}

#[test]
fn test_acp_initialize_params_roundtrip() {
    let params = InitializeParams {
        protocol_version: 1,
        client_info: ClientInfo {
            name: "hone".to_owned(),
            version: "0.1.0".to_owned(),
        },
    };

    let json = serde_json::to_string(&params).expect("serialize");
    assert!(json.contains("protocolVersion"));
    assert!(json.contains("clientInfo"));

    let back: InitializeParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.protocol_version, 1);
    assert_eq!(back.client_info.name, "hone");
}

#[test]
fn test_acp_session_new_result_roundtrip() {
    let result = SessionNewResult {
        session_id: "sess_xyz".to_owned(),
    };

    let json = serde_json::to_string(&result).expect("serialize");
    assert!(json.contains("sessionId"));

    let back: SessionNewResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.session_id, "sess_xyz");
}

// ---------------------------------------------------------------------------
// transport.rs — integration tests using mock agent script
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_acp_transport_initialize() {
    let config = MockAgentConfig {
        agent_name: "test-agent".to_owned(),
        message_chunks: vec!["Hello from agent".to_owned()],
    };
    let (_dir, script_path) = create_mock_agent_script(&config).expect("create script");

    let mut transport = AcpTransport::spawn(script_path.to_str().unwrap(), &[])
        .await
        .expect("spawn transport");

    let params = serde_json::to_value(InitializeParams {
        protocol_version: 1,
        client_info: ClientInfo {
            name: "hone".to_owned(),
            version: "0.1.0".to_owned(),
        },
    })
    .unwrap();

    let result = transport
        .request("initialize", Some(params))
        .await
        .expect("initialize request");

    let protocol_version = result
        .get("protocolVersion")
        .and_then(|v| v.as_u64())
        .expect("protocolVersion in result");

    assert_eq!(protocol_version, 1);
}

#[tokio::test]
async fn test_acp_transport_session_new() {
    let config = MockAgentConfig {
        agent_name: "test-agent".to_owned(),
        message_chunks: vec![],
    };
    let (_dir, script_path) = create_mock_agent_script(&config).expect("create script");

    let mut transport = AcpTransport::spawn(script_path.to_str().unwrap(), &[])
        .await
        .expect("spawn transport");

    // initialize first
    let init_params = serde_json::to_value(InitializeParams {
        protocol_version: 1,
        client_info: ClientInfo {
            name: "hone".to_owned(),
            version: "0.1.0".to_owned(),
        },
    })
    .unwrap();
    transport
        .request("initialize", Some(init_params))
        .await
        .expect("initialize");

    // now session/new
    let session_params = serde_json::to_value(SessionNewParams {
        cwd: "/tmp".to_owned(),
    })
    .unwrap();
    let result = transport
        .request("session/new", Some(session_params))
        .await
        .expect("session/new request");

    let session_id = result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .expect("sessionId in result");

    assert!(!session_id.is_empty(), "sessionId should not be empty");
}

#[tokio::test]
async fn test_acp_transport_notify_does_not_block() {
    let config = MockAgentConfig {
        agent_name: "test-agent".to_owned(),
        message_chunks: vec!["chunk".to_owned()],
    };
    let (_dir, script_path) = create_mock_agent_script(&config).expect("create script");

    let mut transport = AcpTransport::spawn(script_path.to_str().unwrap(), &[])
        .await
        .expect("spawn transport");

    // Sending a notification must not block waiting for a response.
    transport
        .notify(
            "session/cancel",
            Some(serde_json::json!({"sessionId": "sess_abc"})),
        )
        .await
        .expect("notify should not error");
}

// ---------------------------------------------------------------------------
// provider.rs — AcpProvider integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_acp_provider_complete_streams_tokens() {
    let config = MockAgentConfig {
        agent_name: "test-agent".to_owned(),
        message_chunks: vec!["Hello ".to_owned(), "world!".to_owned()],
    };
    let (_dir, script_path) = create_mock_agent_script(&config).expect("create script");

    let provider = AcpProvider::custom(
        script_path.to_str().unwrap(),
        &[],
        "test-acp",
        "Test ACP",
        vec![ModelInfo {
            id: ModelId::new("acp:test-model"),
            display_name: "Test Model".to_owned(),
            context_length: 100_000,
            supports_tools: true,
            supports_streaming: true,
        }],
    );

    let stream = provider.complete(simple_request()).await.expect("complete");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let token_texts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert!(
        !token_texts.is_empty(),
        "expected at least one Token event, got {events:?}"
    );

    let combined = token_texts.join("");
    assert!(
        combined.contains("Hello") || combined.contains("world"),
        "expected text chunks in token events, got: {combined:?}"
    );

    assert!(
        events.contains(&ProviderEvent::Done),
        "expected Done event, got {events:?}"
    );
}

#[tokio::test]
async fn test_acp_provider_done_is_last_event() {
    let config = MockAgentConfig {
        agent_name: "test-agent".to_owned(),
        message_chunks: vec!["some text".to_owned()],
    };
    let (_dir, script_path) = create_mock_agent_script(&config).expect("create script");

    let provider = AcpProvider::custom(
        script_path.to_str().unwrap(),
        &[],
        "test-acp",
        "Test ACP",
        vec![ModelInfo {
            id: ModelId::new("acp:test-model"),
            display_name: "Test Model".to_owned(),
            context_length: 100_000,
            supports_tools: true,
            supports_streaming: true,
        }],
    );

    let stream = provider.complete(simple_request()).await.expect("complete");
    let events: Vec<ProviderEvent> = stream.collect().await;

    assert!(!events.is_empty(), "expected at least one event (Done)");

    let last = events.last().unwrap();
    assert_eq!(
        last,
        &ProviderEvent::Done,
        "Done must be the last event, got {last:?}"
    );
}

#[tokio::test]
async fn test_acp_provider_tool_call_events() {
    // Write a custom mock that emits tool_call / tool_call_update notifications.
    let dir = {
        let unique = format!(
            "hone_mock_tool_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        std::env::temp_dir().join(unique)
    };
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let script_path = dir.join("mock_tool_agent.sh");

    let script = concat!(
        "#!/bin/sh\n",
        "while IFS= read -r line; do\n",
        "    method=$(printf '%s' \"$line\" | sed 's/.*\"method\":\"\\([^\"]*\\)\".*/\\1/')\n",
        "    id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n",
        "    case \"$method\" in\n",
        "        initialize)\n",
        "            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"protocolVersion\":1,\"agentInfo\":{\"name\":\"mock\",\"version\":\"0.1.0\"}}}\\n' \"$id\"\n",
        "            ;;\n",
        "        session/new)\n",
        "            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"sessionId\":\"sess_tool_test\"}}\\n' \"$id\"\n",
        "            ;;\n",
        "        session/prompt)\n",
        "            printf '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"sess_tool_test\",\"update\":{\"sessionUpdate\":\"tool_call\",\"toolCallId\":\"call_001\",\"title\":\"Reading file\",\"kind\":\"read\",\"status\":\"pending\"}}}\\n'\n",
        "            printf '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"sess_tool_test\",\"update\":{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":\"call_001\",\"status\":\"completed\"}}}\\n'\n",
        "            printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"stopReason\":\"end_turn\"}}\\n' \"$id\"\n",
        "            ;;\n",
        "    esac\n",
        "done\n",
    );

    std::fs::write(&script_path, script).expect("write script");
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("set permissions");

    let provider = AcpProvider::custom(
        script_path.to_str().unwrap(),
        &[],
        "test-acp",
        "Test ACP",
        vec![ModelInfo {
            id: ModelId::new("acp:test-model"),
            display_name: "Test Model".to_owned(),
            context_length: 100_000,
            supports_tools: true,
            supports_streaming: true,
        }],
    );

    let stream = provider.complete(simple_request()).await.expect("complete");
    let events: Vec<ProviderEvent> = stream.collect().await;

    let tool_call = events
        .iter()
        .find(|e| matches!(e, ProviderEvent::ToolCall(_)));
    assert!(
        tool_call.is_some(),
        "expected a ToolCall event, got {events:?}"
    );

    if let Some(ProviderEvent::ToolCall(req)) = tool_call {
        assert_eq!(req.id, "call_001");
        assert_eq!(req.name, "Reading file");
    }
}

#[tokio::test]
async fn test_acp_provider_health_when_binary_missing() {
    let provider = AcpProvider::custom(
        "nonexistent_acp_binary_xyz",
        &[],
        "test-acp-missing",
        "Test ACP Missing",
        vec![],
    );

    let health = provider.health().await;
    assert!(
        matches!(health, ProviderHealth::Unavailable { .. }),
        "expected Unavailable, got {health:?}"
    );
}

#[test]
fn test_acp_provider_id_and_name() {
    let provider = AcpProvider::custom("some-binary", &[], "my-acp", "My ACP Provider", vec![]);

    assert_eq!(provider.id(), "my-acp");
    assert_eq!(provider.display_name(), "My ACP Provider");
}

#[test]
fn test_acp_provider_supported_models() {
    let models = vec![
        ModelInfo {
            id: ModelId::new("acp:model-a"),
            display_name: "Model A".to_owned(),
            context_length: 100_000,
            supports_tools: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: ModelId::new("acp:model-b"),
            display_name: "Model B".to_owned(),
            context_length: 200_000,
            supports_tools: true,
            supports_streaming: true,
        },
    ];

    let provider = AcpProvider::custom("binary", &[], "acp", "ACP", models.clone());
    let returned = provider.supported_models();
    assert_eq!(returned.len(), 2);
    assert_eq!(returned[0].id, ModelId::new("acp:model-a"));
    assert_eq!(returned[1].id, ModelId::new("acp:model-b"));
}
