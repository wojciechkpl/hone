//! Cycle 1 — schema announcement (RED phase)
//!
//! These tests define the contract that `AcpProvider::complete()` MUST satisfy
//! after Cycle 1 is implemented: when a `CompletionRequest` carries a non-empty
//! `tools` list, those schemas must appear in at least one of the JSON-RPC
//! messages the provider sends to the ACP agent (the wire location — inside
//! `initialize` params, as a top-level `tools` field on `session/prompt`, etc.
//! — is the implementer's choice; the tests only assert presence of content).
//!
//! All tests are `#[cfg(unix)]`-gated because the mock agent is a shell script.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(unix)]
mod tests {
    use futures::StreamExt;
    use hone_proto::ToolSchema;
    use hone_proto::{ModelId, ModelInfo, ProviderEvent};
    use hone_providers::acp::{AcpProvider, MockAgentConfig, create_mock_agent_script_with_log};
    use hone_providers::{CompletionRequest, ModelProvider};
    use serde_json::json;
    use tempfile::tempdir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a minimal `AcpProvider` that uses the given script binary.
    fn make_provider(script: &str) -> AcpProvider {
        AcpProvider::custom(
            script,
            &[],
            "test-acp-tools",
            "Test ACP (tool schema)",
            vec![ModelInfo {
                id: ModelId::new("acp:test-model"),
                display_name: "Test Model".to_owned(),
                context_length: 100_000,
                supports_tools: true,
                supports_streaming: true,
            }],
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
    // Test 1: tools_are_announced_to_agent
    //
    // Contract: when two ToolSchemas are present in `CompletionRequest.tools`,
    // both tool names must appear somewhere in the JSON-RPC traffic the provider
    // sends to the agent (i.e., in the requests log).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tools_are_announced_to_agent() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let config = MockAgentConfig {
            agent_name: "test-agent".to_owned(),
            message_chunks: vec!["ok".to_owned()],
        };
        let (_script_dir, script_path) =
            create_mock_agent_script_with_log(&config, &log_path).expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        let mut request = CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![hone_proto::Message::user("hello")],
        );
        request.tools = vec![
            ToolSchema {
                name: "read_file".to_owned(),
                description: "Read a file from the filesystem".to_owned(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
            },
            ToolSchema {
                name: "write_file".to_owned(),
                description: "Write content to a file".to_owned(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["path", "content"]
                }),
            },
        ];

        let stream = provider.complete(request).await.expect("complete");
        // Drain the stream so the provider finishes all I/O.
        let _events: Vec<ProviderEvent> = stream.collect().await;

        let log = read_log(&log_path);

        assert!(
            log.contains("read_file"),
            "expected tool name 'read_file' in provider→agent JSON-RPC traffic, got:\n{log}"
        );
        assert!(
            log.contains("write_file"),
            "expected tool name 'write_file' in provider→agent JSON-RPC traffic, got:\n{log}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: empty_tools_omits_schema_announcement
    //
    // Contract: when `request.tools` is empty, no `"tools"` key must appear in
    // any JSON-RPC line sent by the provider (to avoid polluting the wire with
    // an unnecessary empty array).  Both outcomes — key absent, or key present
    // with an empty array — are acceptable; the assertion is deliberately loose
    // so the implementer can choose either convention.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn empty_tools_omits_schema_announcement() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let config = MockAgentConfig {
            agent_name: "test-agent".to_owned(),
            message_chunks: vec!["ok".to_owned()],
        };
        let (_script_dir, script_path) =
            create_mock_agent_script_with_log(&config, &log_path).expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        // Default request has tools = Vec::new().
        let request = CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![hone_proto::Message::user("hello")],
        );

        let stream = provider.complete(request).await.expect("complete");
        let _events: Vec<ProviderEvent> = stream.collect().await;

        let log = read_log(&log_path);

        // Parse every logged line as JSON and check that no line contains a
        // non-empty "tools" array.  An absent key or an empty array both satisfy
        // the contract.
        for (line_no, raw_line) in log.lines().enumerate() {
            if raw_line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(raw_line).unwrap_or(serde_json::Value::Null);

            // Recurse into params if present.
            let params = value.get("params").unwrap_or(&serde_json::Value::Null);

            let tools_in_root = value.get("tools");
            let tools_in_params = params.get("tools");

            for tools_val in [tools_in_root, tools_in_params].into_iter().flatten() {
                if let Some(arr) = tools_val.as_array() {
                    assert!(
                        arr.is_empty(),
                        "line {line_no}: expected no tools or empty tools array when \
                         request.tools is empty, but found non-empty tools: {tools_val}"
                    );
                }
            }
        }
        // Reaching here means the invariant holds (or the log has no "tools"
        // key at all, which is also correct).
    }

    // -----------------------------------------------------------------------
    // Test 3: tool_parameters_field_is_well_formed_json_schema
    //
    // Contract: the `parameters` value of a `ToolSchema` must appear in the
    // wire traffic as a proper JSON object (not double-escaped/stringified).
    // The literal substring `"properties"` must appear in the log, proving
    // the object was serialized inline rather than as an escaped string.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn tool_parameters_field_is_well_formed_json_schema() {
        let tmp = tempdir().expect("tempdir");
        let log_path = tmp.path().join("requests.log");

        let config = MockAgentConfig {
            agent_name: "test-agent".to_owned(),
            message_chunks: vec!["ok".to_owned()],
        };
        let (_script_dir, script_path) =
            create_mock_agent_script_with_log(&config, &log_path).expect("create mock script");

        let provider = make_provider(script_path.to_str().unwrap());

        let mut request = CompletionRequest::new(
            ModelId::new("acp:test-model"),
            vec![hone_proto::Message::user("hello")],
        );
        request.tools = vec![ToolSchema {
            name: "list_directory".to_owned(),
            description: "List files in a directory".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path" }
                },
                "required": ["path"]
            }),
        }];

        let stream = provider.complete(request).await.expect("complete");
        let _events: Vec<ProviderEvent> = stream.collect().await;

        let log = read_log(&log_path);

        // If parameters were double-encoded (stringified), the log would contain
        // `\"properties\"` (escaped) rather than `"properties"` (inline JSON).
        // We assert the unescaped form is present, which proves round-trip
        // fidelity of the serde_json::Value.
        assert!(
            log.contains("\"properties\""),
            "expected the literal JSON key '\"properties\"' in provider→agent traffic \
             (proves ToolSchema.parameters was serialized inline, not double-encoded), \
             got log:\n{log}"
        );

        // Also assert the tool name made it across.
        assert!(
            log.contains("list_directory"),
            "expected tool name 'list_directory' in provider→agent traffic, got:\n{log}"
        );
    }
}
