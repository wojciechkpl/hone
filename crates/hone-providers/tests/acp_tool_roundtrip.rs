//! Cycle 2 — bidirectional tool round-trip (RED phase)
//!
//! These tests define the contract that `AcpProvider::complete()` MUST satisfy
//! after Cycle 2 is implemented: when a `CompletionRequest` carries a
//! `ToolResultStream`, the provider must:
//!
//!   1. Surface a `session/update` tool_call notification as `ProviderEvent::ToolCall`.
//!   2. Pull the corresponding `ToolResult` from the `ToolResultStream`.
//!   3. Send a `session/toolResult` JSON-RPC request to the agent with the result.
//!   4. Continue draining notifications until `session/prompt` returns.
//!
//! All tests are `#[cfg(unix)]`-gated because the mock agent is a shell script.
//!
//! **This file MUST NOT compile until the rust-expert implements
//! `ToolResultStream`, `ToolResultSender`, and `CompletionRequest::tool_results`
//! in the green phase.**  The compile failure is the Red-phase proof.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(unix)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use hone_proto::{ModelId, ModelInfo, ProviderEvent, ToolResult};
    use hone_providers::acp::MockAgentConfig;
    use hone_providers::acp::create_mock_agent_script;
    use hone_providers::acp::{
        AcpProvider, MockToolCallConfig, create_mock_acp_agent_with_tool_call,
    };
    // These two imports will be unresolved until the green phase adds them.
    use hone_providers::traits::{CompletionRequest, ModelProvider, ToolResultStream};
    use serde_json::json;
    use tempfile::tempdir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a minimal `AcpProvider` backed by the given script binary.
    fn make_provider(script: &str) -> AcpProvider {
        AcpProvider::custom(
            script,
            &[],
            "test-acp-toolrt",
            "Test ACP (tool roundtrip)",
            vec![ModelInfo {
                id: ModelId::new("acp:test-model"),
                display_name: "Test Model".to_owned(),
                context_length: 100_000,
                supports_tools: true,
                supports_streaming: true,
            }],
        )
    }

    /// Build a `CompletionRequest` with the test model and a single user message.
    fn make_request(provider: &AcpProvider, text: &str) -> CompletionRequest {
        let _ = provider; // provider supplies model id via supported_models()
        CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![hone_proto::Message::user(text)],
        )
    }

    /// Read the requests log file and return its full content as a `String`.
    /// Panics with a clear message if the file is missing or unreadable.
    fn read_log(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!(
                "requests log not found at {}: {e}\n\
                 This means AcpProvider never wrote any JSON-RPC lines to the \
                 mock agent, or the log path was not passed through.",
                path.display()
            )
        })
    }

    // -----------------------------------------------------------------------
    // Test 1: tool_call_emits_provider_event_then_pulls_result
    //
    // Contract: when the mock agent emits a `tool_call` notification mid-prompt,
    // the provider must:
    //   (a) emit `ProviderEvent::ToolCall` with the correct id,
    //   (b) pull a `ToolResult` from the attached `ToolResultStream`,
    //   (c) forward that result to the agent as a `session/toolResult` request,
    //   (d) continue draining notifications and produce text tokens for both
    //       pre- and post-tool chunks,
    //   (e) terminate with `ProviderEvent::Done`.
    //
    // The mock agent's request log is used as ground truth that (c) happened.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_call_emits_provider_event_then_pulls_result() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let tool_config = MockToolCallConfig {
            agent_name: "tool-rt-agent".to_owned(),
            pre_tool_chunk: "thinking...".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_1".to_owned(),
            tool_call_title: "fs.read".to_owned(),
            raw_input: None,
            tool_call_update_status: None,
        };
        let (_script_dir, script_path) =
            create_mock_acp_agent_with_tool_call(&tool_config, &log_path)
                .expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        // Build the stream + sender pair.  The sender is given to a background
        // task that waits briefly then delivers the ToolResult.  The stream is
        // attached to the CompletionRequest.
        let (rx_stream, tx) = ToolResultStream::new();

        tokio::spawn(async move {
            // Give the provider a moment to emit the ToolCall event before we
            // supply the result.  50 ms is enough for a local shell script.
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(ToolResult {
                tool_call_id: "t_1".to_owned(),
                content: json!({"text": "file contents here"}),
                is_error: false,
            })
            .await
            .expect("send tool result");
        });

        let mut request = make_request(&provider, "read a file");
        request.tool_results = Some(rx_stream);

        let stream = provider.complete(request).await.expect("complete");
        let events: Vec<ProviderEvent> = stream.collect().await;

        // (a) Provider must have surfaced a ToolCall event with id "t_1".
        let tool_call_event = events
            .iter()
            .find(|e| matches!(e, ProviderEvent::ToolCall(req) if req.id == "t_1"));
        assert!(
            tool_call_event.is_some(),
            "expected ProviderEvent::ToolCall with id \"t_1\", got events: {events:?}"
        );

        // (d) At least two Token events: one before and one after the tool call.
        let token_count = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Token { .. }))
            .count();
        assert!(
            token_count >= 2,
            "expected at least 2 Token events (pre- and post-tool chunks), got {token_count} in {events:?}"
        );

        // (e) Done must be the last event.
        assert_eq!(
            events.last().unwrap(),
            &ProviderEvent::Done,
            "Done must be the last event, got {events:?}"
        );

        // (c) The mock agent's log must contain a session/toolResult line with
        //     the correct tool_call_id and the result content.
        let log = read_log(&log_path);
        assert!(
            log.contains("session/toolResult"),
            "expected provider to send a 'session/toolResult' request to the agent, \
             but it was absent from the request log:\n{log}"
        );
        assert!(
            log.contains("t_1"),
            "expected tool_call_id 't_1' in session/toolResult, got log:\n{log}"
        );
        assert!(
            log.contains("file contents here"),
            "expected result content 'file contents here' in session/toolResult, got log:\n{log}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: tool_result_content_is_inline_json_not_string
    //
    // Contract: the `content` field of `ToolResult` must be serialized as an
    // inline JSON value in the `session/toolResult` wire message — not as a
    // double-encoded string.  We prove this by asserting a key from the content
    // object appears literally in the request log.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_result_content_is_inline_json_not_string() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let tool_config = MockToolCallConfig {
            agent_name: "tool-rt-agent".to_owned(),
            pre_tool_chunk: "working".to_owned(),
            post_tool_chunk: "finished".to_owned(),
            tool_call_id: "t_2".to_owned(),
            tool_call_title: "list_items".to_owned(),
            raw_input: None,
            tool_call_update_status: None,
        };
        let (_script_dir, script_path) =
            create_mock_acp_agent_with_tool_call(&tool_config, &log_path)
                .expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        let (rx_stream, tx) = ToolResultStream::new();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(ToolResult {
                tool_call_id: "t_2".to_owned(),
                // Content is a nested JSON object — must not be double-encoded.
                content: json!({"items": [1, 2, 3]}),
                is_error: false,
            })
            .await
            .expect("send tool result");
        });

        let mut request = make_request(&provider, "list items");
        request.tool_results = Some(rx_stream);

        let stream = provider.complete(request).await.expect("complete");
        let _events: Vec<ProviderEvent> = stream.collect().await;

        let log = read_log(&log_path);

        // If `content` were double-encoded (stringified), the log would contain
        // `\"items\"` (escaped) rather than `"items"` (inline JSON key).
        // Asserting the unescaped form proves inline serialization.
        assert!(
            log.contains("\"items\""),
            "expected the literal JSON key '\"items\"' in session/toolResult content \
             (proves content was serialized inline, not double-encoded), got log:\n{log}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: completion_request_with_no_tool_results_still_works
    //
    // Contract: a `CompletionRequest` that omits `tool_results` (i.e. `None`)
    // must drive a prompt that contains no tool_call notifications to completion
    // without error.  This guards backward compatibility: requests that do not
    // opt into tool dispatch must still work end-to-end.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn completion_request_with_no_tool_results_still_works() {
        let config = MockAgentConfig {
            agent_name: "compat-agent".to_owned(),
            message_chunks: vec!["hello".to_owned(), " world".to_owned()],
        };
        let (_script_dir, script_path) =
            create_mock_agent_script(&config).expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        // Deliberately omit tool_results — defaults to None.
        let request = make_request(&provider, "just a normal prompt");
        // Confirm the default is None (would fail to compile if field missing).
        assert!(
            request.tool_results.is_none(),
            "CompletionRequest::new must default tool_results to None"
        );

        let stream = provider.complete(request).await.expect("complete");
        let events: Vec<ProviderEvent> = stream.collect().await;

        // Must not contain any Error events.
        let error_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Error { .. }))
            .collect();
        assert!(
            error_events.is_empty(),
            "expected no Error events for a request without tool_results, got {error_events:?}"
        );

        // Must contain at least one Token and must end with Done.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderEvent::Token { .. })),
            "expected Token events, got {events:?}"
        );
        assert_eq!(
            events.last().unwrap(),
            &ProviderEvent::Done,
            "Done must be the last event, got {events:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: dropping_sender_does_not_hang_provider
    //
    // Contract: if the `ToolResultSender` is dropped before the provider drains
    // the tool call, `ToolResultStream::recv()` must return `None`.  The
    // provider must then emit `ProviderEvent::Error` mentioning the closed
    // result channel, followed by `ProviderEvent::Done` — it must NOT hang.
    //
    // The entire collect is wrapped in a 5-second timeout to guarantee a loud
    // failure rather than a silent hang if this contract is violated.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dropping_sender_does_not_hang_provider() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let tool_config = MockToolCallConfig {
            agent_name: "drop-agent".to_owned(),
            pre_tool_chunk: "searching".to_owned(),
            post_tool_chunk: "done".to_owned(),
            tool_call_id: "t_drop".to_owned(),
            tool_call_title: "db.query".to_owned(),
            raw_input: None,
            tool_call_update_status: None,
        };
        let (_script_dir, script_path) =
            create_mock_acp_agent_with_tool_call(&tool_config, &log_path)
                .expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        // Create the stream + sender pair, then immediately drop the sender.
        // The provider's recv() must see None and surface an Error.
        let (rx_stream, tx) = ToolResultStream::new();
        drop(tx); // sender gone before complete() is called

        let mut request = make_request(&provider, "query the database");
        request.tool_results = Some(rx_stream);

        // Wrap in a 5-second timeout — the test fails loudly if the provider
        // hangs instead of propagating the closed-channel error.
        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let stream = provider.complete(request).await.expect("complete");
            stream.collect::<Vec<ProviderEvent>>().await
        })
        .await
        .expect("provider must not hang when ToolResultSender is dropped (5 s timeout)");

        // Must contain an Error event mentioning the tool result channel.
        let error_event = events.iter().find(|e| {
            if let ProviderEvent::Error { message } = e {
                message.to_lowercase().contains("tool result")
            } else {
                false
            }
        });
        assert!(
            error_event.is_some(),
            "expected ProviderEvent::Error mentioning 'tool result' when sender is dropped, \
             got {events:?}"
        );

        // Done must still be the last event (no silent termination).
        assert_eq!(
            events.last().unwrap(),
            &ProviderEvent::Done,
            "Done must be the last event even after a closed-channel error, got {events:?}"
        );
    }
}
