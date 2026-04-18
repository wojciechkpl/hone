use serde::{Deserialize, Serialize};

/// Which layer a tool belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolLayer {
    /// Layer 2: Native Rust hot-path tools.
    Native,
    /// Layer 1: MCP external integration tools.
    Mcp,
}

/// A request from the model to call a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The result of a tool call, returned to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: serde_json::Value,
    pub is_error: bool,
}

/// JSON Schema definition of a tool, sent to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}
