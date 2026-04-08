use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use hone_sandbox::Sandbox;
use serde_json::Value;

use crate::error::ToolError;

/// Wrapper around raw JSON input passed to a tool.
#[derive(Debug, Clone)]
pub struct ToolInput(pub Value);

impl ToolInput {
    /// Create a new `ToolInput` from a JSON value.
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// Convenience accessor to get a string field by key.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.0.get(key)?.as_str()
    }

    /// Convenience accessor to get an optional u64 field by key.
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.0.get(key)?.as_u64()
    }

    /// Convenience accessor to get an optional bool field by key.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.0.get(key)?.as_bool()
    }
}

/// Wrapper around raw JSON output produced by a tool.
#[derive(Debug, Clone)]
pub struct ToolOutput(pub Value);

impl ToolOutput {
    /// Create a new `ToolOutput` from a JSON value.
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// Consume the wrapper and return the inner JSON value.
    pub fn into_value(self) -> Value {
        self.0
    }
}

/// Execution context passed to every tool call.
#[derive(Clone)]
pub struct ToolContext {
    /// The sandbox that constrains what the tool is allowed to do.
    pub sandbox: Arc<dyn Sandbox>,
    /// The working directory from which relative paths are resolved.
    pub working_dir: PathBuf,
}

/// A native Layer-2 tool that the agent can call directly.
#[async_trait]
pub trait Tool: Send + Sync + 'static {
    /// The unique name used to dispatch calls to this tool.
    fn name(&self) -> &str;

    /// A human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// The JSON Schema for the tool's input parameters.
    fn schema(&self) -> Value;

    /// Execute the tool with the given input and context.
    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;

    /// Which layer this tool belongs to (always [`ToolLayer::Native`] for this crate).
    fn layer(&self) -> ToolLayer;
}
