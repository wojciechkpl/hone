//! Cycle 5 — surface `tool_call_update` as `ObservedToolEnd` (RED phase).
//!
//! When the agent emits a `session/update` notification with
//! `sessionUpdate: "tool_call_update"`, the provider must surface a
//! corresponding `ProviderEvent::ObservedToolEnd` so the dispatcher can:
//!   - correlate the terminal status of in-flight tool calls,
//!   - distinguish success from failure based on the wire status string,
//!   - log / display the outcome in the TUI.
//!
//! Wire contract (this cycle only — `rawOutput` is deferred):
//!   tool_call_update.toolCallId  → ObservedToolEnd.id
//!   tool_call_update.status      → ObservedToolEnd.result (verbatim)
//!   status ∈ {"failed","error"}  → ObservedToolEnd.is_error = true (case-insensitive)
//!   any other status             → ObservedToolEnd.is_error = false
//!
//! All tests are `#[cfg(unix)]`-gated because the mock agent is a shell script.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use hone_proto::{ModelId, ModelInfo, ProviderEvent, ToolResult};
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
            "test-acp-update",
            "Test ACP (tool update)",
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

    /// Drive the round-trip and collect every event.  The mock agent emits a
    /// `tool_call_update` notification (with the configured status) **after**
    /// it has received the provider's `session/toolResult` and **before** it
    /// sends the prompt response, so consumers see the update inside the
    /// stream rather than after `Done`.
    async fn drive(tool_config: MockToolCallConfig) -> Vec<ProviderEvent> {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");
        let (_dir, script_path) = create_mock_acp_agent_with_tool_call(&tool_config, &log_path)
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

        let mut request = make_request("invoke a tool");
        request.tool_results = Some(rx_stream);

        let stream = provider.complete(request).await.expect("complete");
        stream.collect::<Vec<ProviderEvent>>().await
    }

    // -----------------------------------------------------------------------
    // Test 1: completed status → ObservedToolEnd { is_error: false }
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_update_with_completed_status_emits_observed_tool_end() {
        let cfg = MockToolCallConfig {
            agent_name: "tool-update-agent".to_owned(),
            pre_tool_chunk: "thinking".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_upd_1".to_owned(),
            tool_call_title: "fs.read".to_owned(),
            raw_input: None,
            tool_call_update_status: Some("completed".to_owned()),
        };

        let events = drive(cfg).await;

        let observed_end = events.iter().find_map(|e| match e {
            ProviderEvent::ObservedToolEnd {
                id,
                result,
                is_error,
            } if id == "t_upd_1" => Some((result.clone(), *is_error)),
            _ => None,
        });

        assert!(
            observed_end.is_some(),
            "expected ProviderEvent::ObservedToolEnd for id 't_upd_1', got {events:?}"
        );
        let (result, is_error) = observed_end.unwrap();
        assert_eq!(
            result, "completed",
            "ObservedToolEnd.result must mirror the wire status verbatim"
        );
        assert!(
            !is_error,
            "status \"completed\" must NOT be treated as an error"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: failed status → ObservedToolEnd { is_error: true }
    // Mixed-case "Failed" must also be detected.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_update_with_failed_status_marks_is_error() {
        let cfg = MockToolCallConfig {
            agent_name: "fail-agent".to_owned(),
            pre_tool_chunk: "thinking".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_upd_2".to_owned(),
            tool_call_title: "fs.write".to_owned(),
            raw_input: None,
            tool_call_update_status: Some("Failed".to_owned()),
        };

        let events = drive(cfg).await;

        let observed_end = events.iter().find_map(|e| match e {
            ProviderEvent::ObservedToolEnd {
                id,
                result,
                is_error,
            } if id == "t_upd_2" => Some((result.clone(), *is_error)),
            _ => None,
        });

        assert!(
            observed_end.is_some(),
            "expected ProviderEvent::ObservedToolEnd for id 't_upd_2', got {events:?}"
        );
        let (result, is_error) = observed_end.unwrap();
        assert_eq!(
            result, "Failed",
            "ObservedToolEnd.result must preserve the wire status verbatim, including case"
        );
        assert!(
            is_error,
            "status \"Failed\" must be treated as an error (case-insensitive match)"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: ObservedToolEnd is emitted BEFORE Done and AFTER ToolCall.
    // Ensures the event ordering in the stream is preserved so the dispatcher
    // can correlate the lifecycle.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn observed_tool_end_lands_between_tool_call_and_done() {
        let cfg = MockToolCallConfig {
            agent_name: "ordering-agent".to_owned(),
            pre_tool_chunk: "a".to_owned(),
            post_tool_chunk: "b".to_owned(),
            tool_call_id: "t_upd_3".to_owned(),
            tool_call_title: "ping".to_owned(),
            raw_input: None,
            tool_call_update_status: Some("completed".to_owned()),
        };

        let events = drive(cfg).await;

        let tool_call_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::ToolCall(_)))
            .unwrap_or_else(|| panic!("expected ToolCall in {events:?}"));
        let tool_end_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::ObservedToolEnd { .. }))
            .unwrap_or_else(|| panic!("expected ObservedToolEnd in {events:?}"));
        let done_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::Done))
            .unwrap_or_else(|| panic!("expected Done in {events:?}"));

        assert!(
            tool_call_pos < tool_end_pos,
            "ToolCall (idx {tool_call_pos}) must precede ObservedToolEnd (idx {tool_end_pos}) in {events:?}"
        );
        assert!(
            tool_end_pos < done_pos,
            "ObservedToolEnd (idx {tool_end_pos}) must precede Done (idx {done_pos}) in {events:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: omitting tool_call_update keeps the legacy behaviour — no
    // ObservedToolEnd is emitted.  This guards backwards compatibility for
    // agents that never send the update notification.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn missing_tool_call_update_does_not_emit_observed_tool_end() {
        let cfg = MockToolCallConfig {
            agent_name: "legacy-agent".to_owned(),
            pre_tool_chunk: "a".to_owned(),
            post_tool_chunk: "b".to_owned(),
            tool_call_id: "t_upd_4".to_owned(),
            tool_call_title: "tool".to_owned(),
            raw_input: None,
            tool_call_update_status: None,
        };

        let events = drive(cfg).await;

        let has_end = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::ObservedToolEnd { .. }));
        assert!(
            !has_end,
            "no ObservedToolEnd should be emitted when the agent omits tool_call_update; got {events:?}"
        );
    }
}
