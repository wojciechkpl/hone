use std::sync::Arc;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
use serde_json::Value;

use crate::traits::{McpClient, McpToolDef};

/// Adapts an MCP tool definition into the native [`Tool`] trait.
///
/// Multiple `McpTool` instances produced from the same server share a single
/// `Arc<dyn McpClient>` so the underlying connection is not duplicated.
pub struct McpTool {
    /// Retained for diagnostics and future transport routing.
    #[allow(dead_code)]
    server_name: String,
    def: McpToolDef,
    client: Arc<dyn McpClient>,
}

impl McpTool {
    /// Create a new wrapper around a single MCP tool definition.
    pub fn new(server_name: String, def: McpToolDef, client: Arc<dyn McpClient>) -> Self {
        Self {
            server_name,
            def,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> &str {
        &self.def.description
    }

    fn schema(&self) -> Value {
        self.def.input_schema.clone()
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let result = self
            .client
            .call_tool(&self.def.name, input.0)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                tool: self.def.name.clone(),
                message: e.to_string(),
            })?;

        Ok(ToolOutput::new(result))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Mcp
    }
}
