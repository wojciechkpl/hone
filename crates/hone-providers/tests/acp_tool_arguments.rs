//! Cycle 4 — parse `tool_call` arguments correctly (RED phase).
//!
//! These tests pin down the contract for the `rawInput` (a.k.a. arguments)
//! field of an ACP `tool_call` notification:
//!
//!   1. When the mock agent emits a `tool_call` with `rawInput: {...}`,
//!      `ProviderEvent::ToolCall.arguments` MUST equal that JSON object.
//!   2. When `rawInput` is absent, `arguments` MUST default to an empty JSON
//!      object — preserving the previous (Cycle 2) behaviour for backwards
//!      compatibility.
//!   3. Nested / complex `rawInput` payloads MUST round-trip verbatim — no
//!      lossy re-encoding.
//!
//! All tests are `#[cfg(unix)]`-gated because the mock agent is a shell script.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use hone_proto::{ModelId, ModelInfo, ProviderEvent, ToolCallRequest, ToolResult};
    use hone_providers::acp::{
        AcpProvider, MockToolCallConfig, create_mock_acp_agent_with_tool_call,
    };
    use hone_providers::traits::{CompletionRequest, ModelProvider, ToolResultStream};
    use serde_json::json;
    use tempfile::tempdir;

    fn make_provider(script: &str) -> AcpProvider {
        AcpProvider::custom(
            script,
            &[],
            "test-acp-args",
            "Test ACP (tool args)",
            vec![ModelInfo {
                id: ModelId::new("acp:test-model"),
                display_name: "Test Model".to_owned(),
                context_length: 100_000,
                supports_tools: true,
                supports_streaming: true,
            }],
        )
    }

    fn make_request(text: &str) -> CompletionRequest {
        CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![hone_proto::Message::user(text)],
        )
    }

    /// Drive a single round-trip end-to-end and return the `ToolCall` event.
    /// Panics with a descriptive message if no `ToolCall` event is observed.
    async fn drive_and_extract_tool_call(
        tool_config: MockToolCallConfig,
        log_path: &std::path::Path,
    ) -> ToolCallRequest {
        let (_dir, script_path) = create_mock_acp_agent_with_tool_call(&tool_config, log_path)
            .expect("create mock script");
        let provider = make_provider(script_path.to_str().unwrap());

        let (rx_stream, tx) = ToolResultStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(ToolResult {
                tool_call_id: tool_config.tool_call_id.clone(),
                content: json!({"text": "ok"}),
                is_error: false,
            })
            .await
            .expect("send tool result");
        });

        let mut request = make_request("call a tool");
        request.tool_results = Some(rx_stream);

        let stream = provider.complete(request).await.expect("complete");
        let events: Vec<ProviderEvent> = stream.collect().await;

        events
            .into_iter()
            .find_map(|e| match e {
                ProviderEvent::ToolCall(req) => Some(req),
                _ => None,
            })
            .expect("expected exactly one ProviderEvent::ToolCall in the stream")
    }

    // -----------------------------------------------------------------------
    // Test 1: tool_call_arguments_round_trip
    //
    // The mock agent sends `rawInput: {"path": "/etc/hosts", "depth": 3}`.
    // The provider must surface those exact arguments on the wire.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_arguments_round_trip() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let cfg = MockToolCallConfig {
            agent_name: "args-agent".to_owned(),
            pre_tool_chunk: "thinking".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_args_1".to_owned(),
            tool_call_title: "fs.read".to_owned(),
            raw_input: Some(json!({"path": "/etc/hosts", "depth": 3})),
            tool_call_update_status: None,
        };

        let req = drive_and_extract_tool_call(cfg, &log_path).await;

        assert_eq!(
            req.arguments,
            json!({"path": "/etc/hosts", "depth": 3}),
            "expected ProviderEvent::ToolCall.arguments to mirror the agent's rawInput, got {:?}",
            req.arguments
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: tool_call_without_raw_input_defaults_to_empty_object
    //
    // When the agent omits `rawInput` (legacy behaviour), the provider must
    // emit an empty JSON object as `arguments` — never null, never an
    // unexpected variant.  This locks in the Cycle 2 behaviour.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_without_raw_input_defaults_to_empty_object() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let cfg = MockToolCallConfig {
            agent_name: "no-args-agent".to_owned(),
            pre_tool_chunk: "thinking".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_args_2".to_owned(),
            tool_call_title: "ping".to_owned(),
            raw_input: None,
            tool_call_update_status: None,
        };

        let req = drive_and_extract_tool_call(cfg, &log_path).await;

        assert_eq!(
            req.arguments,
            json!({}),
            "expected arguments to default to an empty JSON object when rawInput is omitted, got {:?}",
            req.arguments
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: tool_call_arguments_nested_payload_preserved_verbatim
    //
    // Complex / nested rawInput must round-trip verbatim — no field
    // re-ordering effects, no lossy coercions.  This guards against an
    // accidental string-based path that would corrupt nested arrays/objects.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_arguments_nested_payload_preserved_verbatim() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let nested = json!({
            "filters": [
                {"key": "lang", "values": ["rust", "python"]},
                {"key": "stars", "values": [100, 200, 300]},
            ],
            "limit": 10,
            "include_archived": false,
        });

        let cfg = MockToolCallConfig {
            agent_name: "nested-args-agent".to_owned(),
            pre_tool_chunk: "thinking".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_args_3".to_owned(),
            tool_call_title: "search".to_owned(),
            raw_input: Some(nested.clone()),
            tool_call_update_status: None,
        };

        let req = drive_and_extract_tool_call(cfg, &log_path).await;

        assert_eq!(
            req.arguments, nested,
            "expected nested rawInput to round-trip verbatim, got {:?}",
            req.arguments
        );
    }
}
