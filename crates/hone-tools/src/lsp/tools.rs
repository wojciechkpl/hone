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
        let file_str = input
            .get_str("file")
            .ok_or_else(|| ToolError::InvalidInput {
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

    fn is_read_only(&self) -> bool {
        true
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
        let file_str = input
            .get_str("file")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'file'".to_string(),
            })?;
        let line = input
            .get_u64("line")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'line'".to_string(),
            })? as u32;
        let col = input
            .get_u64("col")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'col'".to_string(),
            })? as u32;

        let file = ctx.working_dir.join(file_str);
        let hover = self.bridge.hover(&file, line, col).await?;
        Ok(ToolOutput::new(json!({ "hover": hover })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
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
        let file_str = input
            .get_str("file")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'file'".to_string(),
            })?;
        let line = input
            .get_u64("line")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'line'".to_string(),
            })? as u32;
        let col = input
            .get_u64("col")
            .ok_or_else(|| ToolError::InvalidInput {
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

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::MockLspBridge;
    use crate::registry::ToolRegistry;

    fn make_lsp_registry() -> ToolRegistry {
        let bridge: Arc<dyn LspBridge> = Arc::new(MockLspBridge::builder().build());
        ToolRegistry::builder()
            .register(Box::new(LspDiagnosticsTool::new(Arc::clone(&bridge))))
            .register(Box::new(LspHoverTool::new(Arc::clone(&bridge))))
            .register(Box::new(LspCodeActionTool::new(Arc::clone(&bridge))))
            .build()
    }

    /// The registry must expose all three LSP tool names when built with a bridge.
    #[test]
    fn lsp_tools_registered_in_registry() {
        let registry = make_lsp_registry();
        assert!(
            registry.get("lsp_diagnostics").is_some(),
            "lsp_diagnostics must be in registry"
        );
        assert!(
            registry.get("lsp_hover").is_some(),
            "lsp_hover must be in registry"
        );
        assert!(
            registry.get("lsp_code_actions").is_some(),
            "lsp_code_actions must be in registry"
        );
    }

    /// The registry correctly reports its size after registration.
    #[test]
    fn lsp_registry_has_correct_count() {
        let registry = make_lsp_registry();
        assert_eq!(registry.len(), 3, "expected exactly 3 LSP tools");
    }
}
