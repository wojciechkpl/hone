//! Cycle 6 — comprehensive end-to-end ACP tool-dispatch lifecycle.
//!
//! This single integration test exercises every contract introduced across
//! Cycles 1-5 in one round-trip:
//!
//!   * Cycle 1 — schema announcement: `request.tools` makes it onto the wire.
//!   * Cycle 2 — bidirectional tool dispatch: `tool_call` notification →
//!     `ProviderEvent::ToolCall` → caller-supplied `ToolResult` →
//!     `session/toolResult` request to agent.
//!   * Cycle 4 — `rawInput` round-trips into `ProviderEvent::ToolCall.arguments`.
//!   * Cycle 5 — `tool_call_update` notification → `ProviderEvent::ObservedToolEnd`
//!     with verbatim status and case-insensitive error classification.
//!   * Cycle 6 — `supports_tools = true` is advertised by the production
//!     `claude` and `gemini` factory constructors.
//!
//! Plus stream invariants: `ToolCall < ObservedToolEnd < Done`, and at least
//! one `Token` event for both pre-tool and post-tool chunks.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use hone_proto::{
        Message, ModelId, ModelInfo, ProviderEvent, ToolResult, ToolSchema,
    };
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
            "test-acp-full",
            "Test ACP (full lifecycle)",
            vec![ModelInfo {
                id: ModelId::new("acp:test-model"),
                display_name: "Test Model".to_owned(),
                context_length: 100_000,
                supports_tools: true,
                supports_streaming: true,
            }],
        )
    }

    /// Single integration test that drives every ACP tool-dispatch contract
    /// in one round-trip and asserts on the resulting event stream + agent
    /// request log.
    #[tokio::test]
    async fn full_acp_tool_dispatch_lifecycle() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let raw_input_payload = json!({
            "path": "/etc/hosts",
            "depth": 2,
            "filters": ["*.txt", "*.md"],
        });

        let cfg = MockToolCallConfig {
            agent_name: "full-lifecycle-agent".to_owned(),
            pre_tool_chunk: "thinking…".to_owned(),
            post_tool_chunk: "done!".to_owned(),
            tool_call_id: "t_full_1".to_owned(),
            tool_call_title: "fs.read".to_owned(),
            raw_input: Some(raw_input_payload.clone()),
            tool_call_update_status: Some("completed".to_owned()),
        };

        let (_dir, script_path) =
            create_mock_acp_agent_with_tool_call(&cfg, &log_path).expect("create mock script");
        let provider = make_provider(script_path.to_str().unwrap());

        // The dispatched tool result the caller supplies in response to the
        // ToolCall event.  Its content must show up verbatim in the agent's
        // requests log under a `session/toolResult` line.
        let dispatched_result_content = json!({"text": "127.0.0.1 localhost"});

        let (rx_stream, tx) = ToolResultStream::new();
        let result_for_dispatch = ToolResult {
            tool_call_id: cfg.tool_call_id.clone(),
            content: dispatched_result_content.clone(),
            is_error: false,
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(result_for_dispatch).await.expect("send tool result");
        });

        let mut request = CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![Message::user("read /etc/hosts please")],
        );
        request.tools = vec![ToolSchema {
            name: "fs.read".to_owned(),
            description: "Read a file from the filesystem".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "depth": { "type": "integer" },
                },
                "required": ["path"],
            }),
        }];
        request.tool_results = Some(rx_stream);

        let stream = provider.complete(request).await.expect("complete");
        let events: Vec<ProviderEvent> = stream.collect().await;

        // -------------------------------------------------------------------
        // Stream-level assertions
        // -------------------------------------------------------------------

        // (1) ToolCall event with parsed arguments matching the agent's rawInput.
        let tool_call = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::ToolCall(req) => Some(req.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("expected ProviderEvent::ToolCall in {events:?}"));
        assert_eq!(tool_call.id, "t_full_1");
        assert_eq!(tool_call.name, "fs.read");
        assert_eq!(
            tool_call.arguments, raw_input_payload,
            "ProviderEvent::ToolCall.arguments must mirror the agent's rawInput verbatim"
        );

        // (2) ObservedToolEnd event with verbatim status and is_error = false.
        let observed_end = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::ObservedToolEnd { id, result, is_error } if id == "t_full_1" => {
                    Some((result.clone(), *is_error))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("expected ProviderEvent::ObservedToolEnd for 't_full_1' in {events:?}")
            });
        let (result, is_error) = observed_end;
        assert_eq!(result, "completed");
        assert!(!is_error, "completed status must not be flagged as error");

        // (3) Token events for both the pre-tool and post-tool chunks.
        let token_texts: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::Token { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            token_texts.iter().any(|t| t.contains("thinking")),
            "expected a Token event containing the pre-tool chunk; got tokens={token_texts:?}"
        );
        assert!(
            token_texts.iter().any(|t| t.contains("done")),
            "expected a Token event containing the post-tool chunk; got tokens={token_texts:?}"
        );

        // (4) Event ordering: ToolCall < ObservedToolEnd < Done; Done is last.
        let tool_call_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::ToolCall(_)))
            .unwrap();
        let tool_end_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::ObservedToolEnd { .. }))
            .unwrap();
        let done_pos = events
            .iter()
            .position(|e| matches!(e, ProviderEvent::Done))
            .unwrap_or_else(|| panic!("expected Done in {events:?}"));
        assert!(
            tool_call_pos < tool_end_pos,
            "ToolCall (idx {tool_call_pos}) must precede ObservedToolEnd (idx {tool_end_pos})"
        );
        assert!(
            tool_end_pos < done_pos,
            "ObservedToolEnd (idx {tool_end_pos}) must precede Done (idx {done_pos})"
        );
        assert_eq!(
            done_pos,
            events.len() - 1,
            "Done must be the final event in the stream"
        );

        // -------------------------------------------------------------------
        // Wire-level assertions (agent requests log)
        // -------------------------------------------------------------------

        let log = std::fs::read_to_string(&log_path).expect("read log");

        // Cycle 1 — schema announcement: tool name + parameters object made it
        // onto the wire as well-formed JSON (not double-encoded).
        assert!(
            log.contains("fs.read"),
            "expected tool name 'fs.read' in agent request log, got:\n{log}"
        );
        assert!(
            log.contains("\"properties\""),
            "expected ToolSchema.parameters to be inlined as JSON \
             (literal '\"properties\"' key); got:\n{log}"
        );

        // Cycle 2 — provider sent back a session/toolResult containing the
        // dispatched content.
        let tool_result_lines: Vec<&str> = log
            .lines()
            .filter(|l| l.contains("session/toolResult"))
            .collect();
        assert!(
            !tool_result_lines.is_empty(),
            "expected at least one session/toolResult request in agent log, got:\n{log}"
        );
        let any_carries_content = tool_result_lines
            .iter()
            .any(|l| l.contains("127.0.0.1 localhost"));
        assert!(
            any_carries_content,
            "expected the dispatched ToolResult.content to appear in a session/toolResult line; \
             got tool_result_lines={tool_result_lines:?}"
        );

        // -------------------------------------------------------------------
        // Cycle 6 — production factories advertise supports_tools = true
        // -------------------------------------------------------------------
        for (label, models) in [
            ("claude", AcpProvider::claude().supported_models()),
            ("gemini", AcpProvider::gemini().supported_models()),
        ] {
            assert!(
                !models.is_empty(),
                "{label} factory must expose at least one model"
            );
            for model in &models {
                assert!(
                    model.supports_tools,
                    "{label} model {} must advertise supports_tools=true",
                    model.id.as_str()
                );
            }
        }
    }
}
