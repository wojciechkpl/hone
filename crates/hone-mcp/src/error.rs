use thiserror::Error;

/// Errors that can occur when communicating with an MCP server.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("connection to MCP server '{server}' failed: {source}")]
    ConnectionFailed {
        server: String,
        source: std::io::Error,
    },

    #[error("MCP server '{server}' timed out after {timeout_ms}ms")]
    Timeout { server: String, timeout_ms: u64 },

    #[error("MCP protocol error from server '{server}': {message}")]
    ProtocolError { server: String, message: String },

    #[error("tool '{tool}' not found on MCP server '{server}'")]
    ToolNotFound { server: String, tool: String },

    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
