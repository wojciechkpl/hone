use std::sync::Arc;

use hone_mcp::{McpClient, McpError, McpTool, McpToolDef, MockMcpClient, SchemaCache};
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolInput};
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_tool_def(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: format!("Does {name}"),
        input_schema: json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            }
        }),
    }
}

fn make_mock_client() -> MockMcpClient {
    let tools = vec![make_tool_def("search"), make_tool_def("fetch")];
    MockMcpClient::new("test-server", tools)
}

// ---------------------------------------------------------------------------
// connect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mock_client_connect_returns_info() {
    let mut client = make_mock_client();
    let info = client.connect().await.expect("connect should succeed");

    assert_eq!(info.name, "test-server");
    assert_eq!(info.tools.len(), 2);
    assert_eq!(info.tools[0].name, "search");
    assert_eq!(info.tools[1].name, "fetch");
}

// ---------------------------------------------------------------------------
// list_tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mock_client_list_tools() {
    let client = make_mock_client();
    let tools = client
        .list_tools()
        .await
        .expect("list_tools should succeed");

    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|t| t.name == "search"));
    assert!(tools.iter().any(|t| t.name == "fetch"));
}

// ---------------------------------------------------------------------------
// call_tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mock_client_call_tool() {
    let tools = vec![make_tool_def("search")];
    let mut client = MockMcpClient::new("test-server", tools);
    client.with_response("search", json!({"results": ["a", "b"]}));

    let result = client
        .call_tool("search", json!({"input": "query"}))
        .await
        .expect("call_tool should succeed");

    assert_eq!(result, json!({"results": ["a", "b"]}));
}

// ---------------------------------------------------------------------------
// call_tool — unknown tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mock_client_call_unknown_tool_errors() {
    let client = make_mock_client();
    let err = client
        .call_tool("nonexistent", json!({}))
        .await
        .expect_err("should fail for unknown tool");

    match err {
        McpError::ToolNotFound { server, tool } => {
            assert_eq!(server, "test-server");
            assert_eq!(tool, "nonexistent");
        }
        other => panic!("expected ToolNotFound, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// tools_as_native
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tools_as_native_returns_tool_impls() {
    let client = make_mock_client();
    let native_tools = client.tools_as_native();

    assert_eq!(native_tools.len(), 2);
    let names: Vec<&str> = native_tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"search"));
    assert!(names.contains(&"fetch"));
}

// ---------------------------------------------------------------------------
// McpTool::layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_tool_layer_is_mcp() {
    let client = Arc::new(make_mock_client());
    let def = make_tool_def("search");
    let tool = McpTool::new("test-server".to_string(), def, client);

    assert_eq!(tool.layer(), ToolLayer::Mcp);
}

// ---------------------------------------------------------------------------
// McpTool::call delegates to client
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_tool_call_delegates_to_client() {
    use std::path::PathBuf;

    let tools = vec![make_tool_def("search")];
    let mut mock = MockMcpClient::new("test-server", tools);
    mock.with_response("search", json!({"hit": true}));

    let client: Arc<dyn McpClient> = Arc::new(mock);
    let def = make_tool_def("search");
    let tool = McpTool::new("test-server".to_string(), def, Arc::clone(&client));

    // Build a minimal ToolContext using the no-op sandbox from hone-sandbox
    let sandbox = hone_sandbox::NullSandbox;
    let ctx = ToolContext {
        sandbox: Arc::new(sandbox),
        working_dir: PathBuf::from("/tmp"),
    };

    let output = tool
        .call(ToolInput::new(json!({"input": "hello"})), &ctx)
        .await
        .expect("call should succeed");

    assert_eq!(output.into_value(), json!({"hit": true}));
}

// ---------------------------------------------------------------------------
// McpTool::schema matches def
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_tool_schema_matches_def() {
    let client = Arc::new(make_mock_client());
    let def = make_tool_def("search");
    let expected_schema = def.input_schema.clone();
    let tool = McpTool::new("test-server".to_string(), def, client);

    assert_eq!(tool.schema(), expected_schema);
}

// ---------------------------------------------------------------------------
// SchemaCache loads once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_schema_cache_loads_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let cache = SchemaCache::new();

    // Loader closure — tracks how many times it's invoked
    let result1 = cache
        .get_or_load("my-server", || {
            call_count_clone.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, McpError>(vec![make_tool_def("tool-a")]) }
        })
        .await
        .expect("first load should succeed");

    let call_count_clone2 = Arc::clone(&call_count);
    let result2 = cache
        .get_or_load("my-server", || {
            call_count_clone2.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, McpError>(vec![make_tool_def("tool-a")]) }
        })
        .await
        .expect("second load should return cached result");

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "loader must be called exactly once"
    );
    assert_eq!(result1.len(), 1);
    assert_eq!(result2.len(), 1);
    assert_eq!(result1[0].name, "tool-a");
}
