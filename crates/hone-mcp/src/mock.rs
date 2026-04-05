use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use hone_tools::Tool;
use serde_json::Value;

use crate::error::McpError;
use crate::tool_wrapper::McpTool;
use crate::traits::{McpClient, McpServerInfo, McpToolDef};

/// Test double for [`McpClient`].
///
/// Pre-loaded with tool definitions and optional canned responses. Used by
/// `hone-core` integration tests so they do not need a live MCP server.
pub struct MockMcpClient {
    server_name: String,
    tools: Vec<McpToolDef>,
    responses: HashMap<String, Value>,
}

impl MockMcpClient {
    /// Create a new mock client with the given server name and tool definitions.
    pub fn new(server_name: impl Into<String>, tools: Vec<McpToolDef>) -> Self {
        Self {
            server_name: server_name.into(),
            tools,
            responses: HashMap::new(),
        }
    }

    /// Register a canned response for a specific tool name.
    ///
    /// If no response is registered for a tool, [`McpClient::call_tool`] returns
    /// `serde_json::Value::Null` for known tools and [`McpError::ToolNotFound`]
    /// for unknown ones.
    pub fn with_response(&mut self, tool_name: impl Into<String>, response: Value) {
        self.responses.insert(tool_name.into(), response);
    }
}

#[async_trait]
impl McpClient for MockMcpClient {
    async fn connect(&mut self) -> Result<McpServerInfo, McpError> {
        Ok(McpServerInfo {
            name: self.server_name.clone(),
            version: "0.0.0-mock".to_string(),
            tools: self.tools.clone(),
        })
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(&self, name: &str, _args: Value) -> Result<Value, McpError> {
        let known = self.tools.iter().any(|t| t.name == name);
        if !known {
            return Err(McpError::ToolNotFound {
                server: self.server_name.clone(),
                tool: name.to_string(),
            });
        }

        let response = self
            .responses
            .get(name)
            .cloned()
            .unwrap_or(Value::Null);

        Ok(response)
    }

    fn tools_as_native(&self) -> Vec<Box<dyn Tool>> {
        // Wrap self in an Arc so each McpTool can share the same client.
        // Because MockMcpClient is used directly (not behind an Arc) here, we
        // clone the minimum needed data for each wrapper.
        let client: Arc<dyn McpClient> = Arc::new(MockMcpClient {
            server_name: self.server_name.clone(),
            tools: self.tools.clone(),
            responses: self.responses.clone(),
        });

        self.tools
            .iter()
            .map(|def| -> Box<dyn Tool> {
                Box::new(McpTool::new(
                    self.server_name.clone(),
                    def.clone(),
                    Arc::clone(&client),
                ))
            })
            .collect()
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}
