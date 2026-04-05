use async_trait::async_trait;
use hone_tools::Tool;
use serde_json::Value;

use crate::error::McpError;

/// Definition of a single tool advertised by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Information returned by an MCP server during connection handshake.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
    pub tools: Vec<McpToolDef>,
}

/// Abstraction over an MCP server connection.
///
/// Implementors wrap either a real transport (stdio, HTTP) or a test double.
/// Multiple [`crate::McpTool`] instances can share one client via `Arc<dyn McpClient>`.
#[async_trait]
pub trait McpClient: Send + Sync + 'static {
    /// Perform the initial handshake and return server metadata.
    async fn connect(&mut self) -> Result<McpServerInfo, McpError>;

    /// List all tools currently advertised by the server.
    async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError>;

    /// Call a named tool with the provided JSON arguments.
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError>;

    /// Wrap each advertised tool definition as a native [`Tool`] trait object.
    fn tools_as_native(&self) -> Vec<Box<dyn Tool>>;

    /// The name this client was configured with (used for error messages).
    fn server_name(&self) -> &str;
}
