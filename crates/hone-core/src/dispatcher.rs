use std::path::PathBuf;
use std::sync::Arc;

use hone_proto::{ToolCallRequest, ToolResult};
use hone_sandbox::{Sandbox, SandboxPolicy};
use hone_tools::{ToolContext, ToolInput, ToolRegistry};

use crate::error::AgentError;

/// Routes tool call requests to the correct registered tool.
pub struct ToolDispatcher {
    registry: ToolRegistry,
    sandbox: Arc<dyn Sandbox>,
    working_dir: PathBuf,
}

impl ToolDispatcher {
    pub fn new(registry: ToolRegistry, sandbox: Arc<dyn Sandbox>, working_dir: PathBuf) -> Self {
        Self {
            registry,
            sandbox,
            working_dir,
        }
    }

    /// Dispatch a tool call, returning a `ToolResult` ready to be sent back to
    /// the model.  Returns `AgentError::ToolNotFound` if no matching tool is
    /// registered, or `AgentError::Tool` if the tool itself fails.
    pub async fn dispatch(&self, call: &ToolCallRequest) -> Result<ToolResult, AgentError> {
        let tool = self
            .registry
            .get(&call.name)
            .ok_or_else(|| AgentError::ToolNotFound {
                name: call.name.clone(),
            })?;

        let ctx = ToolContext {
            sandbox: Arc::clone(&self.sandbox),
            sandbox_policy: Arc::new(SandboxPolicy::permissive()),
            working_dir: self.working_dir.clone(),
        };

        let input = ToolInput::new(call.arguments.clone());

        match tool.call(input, &ctx).await {
            Ok(output) => Ok(ToolResult {
                tool_call_id: call.id.clone(),
                content: output.into_value(),
                is_error: false,
            }),
            Err(e) => Err(AgentError::Tool {
                name: call.name.clone(),
                source: e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hone_proto::{ToolCallRequest, ToolLayer};
    use hone_sandbox::NullSandbox;
    use hone_tools::{Tool, ToolError, ToolInput, ToolOutput, ToolRegistry};
    use serde_json::json;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes its input back."
        }

        fn schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": { "text": { "type": "string" } } })
        }

        async fn call(
            &self,
            input: ToolInput,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
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

        let result = dispatcher.dispatch(&call).await.unwrap();
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

        let err = dispatcher.dispatch(&call).await.unwrap_err();
        assert!(matches!(
            err,
            AgentError::ToolNotFound { name } if name == "nonexistent"
        ));
    }
}
