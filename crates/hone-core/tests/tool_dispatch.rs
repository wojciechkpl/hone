use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hone_core::{AgentError, ToolDispatcher};
use hone_proto::{ToolCallRequest, ToolLayer};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::json;

/// A simple tool that echoes its input back unchanged.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes its JSON input back."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            }
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(input.0))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

fn make_dispatcher() -> ToolDispatcher {
    let registry = ToolRegistry::builder().register(Box::new(EchoTool)).build();
    ToolDispatcher::new(registry, Arc::new(NullSandbox), PathBuf::from("/tmp"))
}

#[tokio::test]
async fn test_dispatch_to_registered_tool() {
    let dispatcher = make_dispatcher();

    let call = ToolCallRequest {
        id: "call-1".to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "text": "hello world" }),
    };

    let result = dispatcher
        .dispatch(&call)
        .await
        .expect("dispatch should succeed");
    assert_eq!(result.tool_call_id, "call-1");
    assert!(!result.is_error);
    assert_eq!(result.content, json!({ "text": "hello world" }));
}

#[tokio::test]
async fn test_dispatch_unknown_tool_errors() {
    let dispatcher = make_dispatcher();

    let call = ToolCallRequest {
        id: "call-2".to_owned(),
        name: "nonexistent".to_owned(),
        arguments: json!({}),
    };

    let err = dispatcher
        .dispatch(&call)
        .await
        .expect_err("should fail for unknown tool");

    assert!(
        matches!(err, AgentError::ToolNotFound { ref name } if name == "nonexistent"),
        "expected ToolNotFound, got {err:?}"
    );
}
