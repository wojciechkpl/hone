use std::sync::Arc;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use super::bridge::LspBridge;
use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

// ---------------------------------------------------------------------------
// LspDiagnosticsTool
// ---------------------------------------------------------------------------

/// Returns all LSP diagnostics for a file as a JSON array.
pub struct LspDiagnosticsTool {
    bridge: Arc<dyn LspBridge>,
}

impl LspDiagnosticsTool {
    pub fn new(bridge: Arc<dyn LspBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        "Return all LSP diagnostics (errors, warnings, hints) for a file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file to diagnose."
                }
            },
            "required": ["file"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let file_str = input.get_str("file").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'file'".to_string(),
        })?;
        let file = ctx.working_dir.join(file_str);
        let diagnostics = self.bridge.diagnostics(&file).await?;
        let value = serde_json::to_value(&diagnostics).map_err(|e| ToolError::ExecutionFailed {
            tool: self.name().to_string(),
            message: e.to_string(),
        })?;
        Ok(ToolOutput::new(value))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// LspHoverTool
// ---------------------------------------------------------------------------

/// Returns hover documentation for a position in a file.
pub struct LspHoverTool {
    bridge: Arc<dyn LspBridge>,
}

impl LspHoverTool {
    pub fn new(bridge: Arc<dyn LspBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn description(&self) -> &str {
        "Return hover documentation at a specific position in a file, or null if none."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file."
                },
                "line": {
                    "type": "integer",
                    "description": "Zero-based line number.",
                    "minimum": 0
                },
                "col": {
                    "type": "integer",
                    "description": "Zero-based column number.",
                    "minimum": 0
                }
            },
            "required": ["file", "line", "col"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let file_str = input.get_str("file").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'file'".to_string(),
        })?;
        let line = input.get_u64("line").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'line'".to_string(),
        })? as u32;
        let col = input.get_u64("col").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'col'".to_string(),
        })? as u32;

        let file = ctx.working_dir.join(file_str);
        let hover = self.bridge.hover(&file, line, col).await?;
        Ok(ToolOutput::new(json!({ "hover": hover })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// LspCodeActionTool
// ---------------------------------------------------------------------------

/// Returns code actions available at a position in a file.
pub struct LspCodeActionTool {
    bridge: Arc<dyn LspBridge>,
}

impl LspCodeActionTool {
    pub fn new(bridge: Arc<dyn LspBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl Tool for LspCodeActionTool {
    fn name(&self) -> &str {
        "lsp_code_actions"
    }

    fn description(&self) -> &str {
        "Return code actions available at a specific position in a file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file."
                },
                "line": {
                    "type": "integer",
                    "description": "Zero-based line number.",
                    "minimum": 0
                },
                "col": {
                    "type": "integer",
                    "description": "Zero-based column number.",
                    "minimum": 0
                }
            },
            "required": ["file", "line", "col"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let file_str = input.get_str("file").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'file'".to_string(),
        })?;
        let line = input.get_u64("line").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'line'".to_string(),
        })? as u32;
        let col = input.get_u64("col").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'col'".to_string(),
        })? as u32;

        let file = ctx.working_dir.join(file_str);
        let actions = self.bridge.code_actions(&file, line, col).await?;
        let value = serde_json::to_value(&actions).map_err(|e| ToolError::ExecutionFailed {
            tool: self.name().to_string(),
            message: e.to_string(),
        })?;
        Ok(ToolOutput::new(value))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
