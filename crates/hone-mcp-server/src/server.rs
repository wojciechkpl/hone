use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hone_tools::{ToolContext, ToolRegistry};

#[allow(unused_imports)]
use crate::jsonrpc::{JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse};

// ---------------------------------------------------------------------------
// ServerInfo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            name: "hone-mcp-server".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: "2024-11-05".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// McpServerError
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum McpServerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

pub struct McpServer {
    registry: ToolRegistry,
    ctx: ToolContext,
    info: ServerInfo,
    initialized: AtomicBool,
}

impl McpServer {
    pub fn new(registry: ToolRegistry, ctx: ToolContext) -> Self {
        Self {
            registry,
            ctx,
            info: ServerInfo::default(),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn with_server_info(self, info: ServerInfo) -> Self {
        Self { info, ..self }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        // Validate JSON-RPC version. Only respond for requests (id present).
        if req.jsonrpc != "2.0" {
            return req.id.map(|id| {
                JsonRpcResponse::err(id, JsonRpcError::invalid_request(r#"jsonrpc must be "2.0""#))
            });
        }

        let method = req.method.as_str();

        // Notifications (no id) — handle side-effects, never respond.
        if req.id.is_none() {
            if method == "notifications/initialized" {
                self.initialized.store(true, Ordering::Relaxed);
            }
            // All other notifications (known or unknown) are silently dropped.
            return None;
        }

        let id = req.id.clone().unwrap();

        match method {
            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": self.info.protocol_version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": self.info.name, "version": self.info.version }
                });
                Some(JsonRpcResponse::ok(id, result))
            }

            "tools/list" => {
                let tools: Vec<serde_json::Value> = self
                    .registry
                    .schemas()
                    .into_iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "description": s.description,
                            "inputSchema": s.parameters
                        })
                    })
                    .collect();
                Some(JsonRpcResponse::ok(id, serde_json::json!({ "tools": tools })))
            }

            "tools/call" => {
                let params = req.params.unwrap_or(serde_json::json!({}));
                let name = match params["name"].as_str() {
                    Some(n) => n.to_string(),
                    None => {
                        return Some(JsonRpcResponse::err(
                            id,
                            JsonRpcError::invalid_params("missing 'name' field"),
                        ));
                    }
                };
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                match self.registry.get(&name) {
                    None => Some(JsonRpcResponse::err(
                        id,
                        JsonRpcError::invalid_params(format!("tool not found: {name}")),
                    )),
                    Some(tool) => {
                        use hone_tools::ToolInput;
                        let tool = Arc::clone(tool);
                        match tool.call(ToolInput::new(arguments), &self.ctx).await {
                            Ok(output) => {
                                let text = serde_json::to_string(&output.into_value())
                                    .unwrap_or_else(|_| "<serialization error>".into());
                                Some(JsonRpcResponse::ok(
                                    id,
                                    serde_json::json!({
                                        "content": [{ "type": "text", "text": text }],
                                        "isError": false
                                    }),
                                ))
                            }
                            Err(e) => Some(JsonRpcResponse::ok(
                                id,
                                serde_json::json!({
                                    "content": [{ "type": "text", "text": e.to_string() }],
                                    "isError": true
                                }),
                            )),
                        }
                    }
                }
            }

            other => {
                if other.starts_with("notifications/") {
                    None
                } else {
                    Some(JsonRpcResponse::err(
                        id,
                        JsonRpcError::method_not_found(format!("unknown method: {other}")),
                    ))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hone_proto::ToolLayer;
    use hone_sandbox::NullSandbox;
    use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Minimal mock tools
    // -----------------------------------------------------------------------

    struct OkTool {
        name: &'static str,
        description: &'static str,
    }

    #[async_trait]
    impl Tool for OkTool {
        fn name(&self) -> &str { self.name }
        fn description(&self) -> &str { self.description }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": { "key": { "type": "string" } } })
        }
        fn layer(&self) -> ToolLayer { ToolLayer::Native }
        async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(json!({ "hello": "world" })))
        }
    }

    struct ErrTool;

    #[async_trait]
    impl Tool for ErrTool {
        fn name(&self) -> &str { "err_tool" }
        fn description(&self) -> &str { "always fails" }
        fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
        fn layer(&self) -> ToolLayer { ToolLayer::Native }
        async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
            Err(ToolError::Other("intentional failure".into()))
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_ctx() -> ToolContext {
        ToolContext {
            sandbox: Arc::new(NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            working_dir: PathBuf::from("/tmp"),
        }
    }

    fn make_registry_with_mock_tools() -> ToolRegistry {
        ToolRegistry::builder()
            .register(Box::new(OkTool { name: "mock_a", description: "first mock" }))
            .register(Box::new(OkTool { name: "mock_b", description: "second mock" }))
            .build()
    }

    fn make_server() -> McpServer {
        McpServer::new(make_registry_with_mock_tools(), make_ctx())
    }

    fn request(id: i64, method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(JsonRpcId::Number(id)),
            method: method.into(),
            params,
        }
    }

    fn notification(method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: method.into(),
            params: None,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn handle_request_initialize_returns_server_info() {
        let server = make_server();
        let req = request(1, "initialize", Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0" }
        })));

        let resp = server.handle_request(req).await.expect("should return response");
        let result = resp.result.expect("result must be present");

        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "hone-mcp-server");
    }

    #[tokio::test]
    async fn handle_request_notifications_initialized_returns_none_and_marks_initialized() {
        let server = make_server();
        assert!(!server.is_initialized());

        let notif = notification("notifications/initialized");
        let resp = server.handle_request(notif).await;

        assert!(resp.is_none(), "notifications must not produce a response");
        assert!(server.is_initialized());
    }

    #[tokio::test]
    async fn handle_request_tools_list_returns_registered_tools() {
        let server = make_server();
        let req = request(2, "tools/list", None);
        let resp = server.handle_request(req).await.expect("should return response");
        let result = resp.result.expect("result must be present");
        let tools = result["tools"].as_array().expect("tools must be an array");

        assert_eq!(tools.len(), 2);
    }

    #[tokio::test]
    async fn handle_request_tools_list_uses_inputSchema_key_not_parameters() {
        let server = make_server();
        let req = request(2, "tools/list", None);
        let resp = server.handle_request(req).await.expect("should return response");
        let result = resp.result.expect("result must be present");
        let tools = result["tools"].as_array().expect("tools must be an array");

        for tool in tools {
            assert!(
                tool.get("inputSchema").is_some(),
                "wire format must use 'inputSchema' key"
            );
            assert!(
                tool.get("parameters").is_none(),
                "wire format must NOT use 'parameters' key"
            );
        }
    }

    #[tokio::test]
    async fn handle_request_tools_call_invokes_tool_and_returns_content() {
        let server = make_server();
        let req = request(3, "tools/call", Some(json!({
            "name": "mock_a",
            "arguments": { "key": "value" }
        })));

        let resp = server.handle_request(req).await.expect("should return response");
        let result = resp.result.expect("result must be present");

        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().expect("content must be an array");
        assert!(!content.is_empty());
        assert_eq!(content[0]["type"], "text");

        let text = content[0]["text"].as_str().expect("text must be a string");
        let parsed: Value = serde_json::from_str(text).expect("text must be valid JSON");
        assert_eq!(parsed["hello"], "world");
    }

    #[tokio::test]
    async fn handle_request_tools_call_unknown_tool_returns_invalid_params_error() {
        let server = make_server();
        let req = request(4, "tools/call", Some(json!({ "name": "ghost" })));

        let resp = server.handle_request(req).await.expect("should return response");
        assert!(resp.result.is_none(), "must be an error response, not a result");
        let err = resp.error.expect("error must be present");

        assert_eq!(err.code, JsonRpcError::INVALID_PARAMS);
        assert!(err.message.contains("ghost"), "error message should mention the tool name");
    }

    #[tokio::test]
    async fn handle_request_tools_call_missing_arguments_uses_empty_object() {
        let server = make_server();
        // Omit "arguments" key — server should default to {} and still dispatch
        let req = request(5, "tools/call", Some(json!({ "name": "mock_a" })));

        let resp = server.handle_request(req).await.expect("should return response");
        let result = resp.result.expect("result must be present");
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn handle_request_tools_call_tool_error_returns_isError_true() {
        let registry = ToolRegistry::builder()
            .register(Box::new(ErrTool))
            .build();
        let server = McpServer::new(registry, make_ctx());
        let req = request(6, "tools/call", Some(json!({ "name": "err_tool", "arguments": {} })));

        let resp = server.handle_request(req).await.expect("should return response");
        // MCP convention: tool errors are NOT JSON-RPC errors — the call itself
        // succeeded; the tool returned an application-level error.
        assert!(
            resp.error.is_none(),
            "must NOT be a JSON-RPC error; tool errors live in result.isError"
        );
        let result = resp.result.expect("result must be present");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().expect("text must be present");
        assert!(text.contains("intentional failure"));
    }

    #[tokio::test]
    async fn handle_request_unknown_method_returns_method_not_found() {
        let server = make_server();
        let req = request(7, "made/up", None);
        let resp = server.handle_request(req).await.expect("should return response");
        let err = resp.error.expect("error must be present");
        assert_eq!(err.code, JsonRpcError::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn handle_request_request_with_wrong_jsonrpc_version_returns_invalid_request() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "1.0".into(),
            id: Some(JsonRpcId::Number(8)),
            method: "tools/list".into(),
            params: None,
        };
        let resp = server.handle_request(req).await.expect("should return response");
        let err = resp.error.expect("error must be present");
        assert_eq!(err.code, JsonRpcError::INVALID_REQUEST);
    }
}
