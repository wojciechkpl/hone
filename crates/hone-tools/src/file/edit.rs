use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Performs an exact string replacement in a file.
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace the first occurrence of `old_str` with `new_str` in the specified file. \
         Returns `applied: false` when `old_str` is not found."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit."
                },
                "old_str": {
                    "type": "string",
                    "description": "The exact string to search for."
                },
                "new_str": {
                    "type": "string",
                    "description": "The replacement string."
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path_str = input.get_str("path").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'path'".to_string(),
        })?;

        let old_str = input
            .get_str("old_str")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'old_str'".to_string(),
            })?;

        let new_str = input
            .get_str("new_str")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'new_str'".to_string(),
            })?;

        let path = ctx.working_dir.join(path_str);
        let original = tokio::fs::read_to_string(&path).await?;

        if !original.contains(old_str) {
            let path_out = path.to_string_lossy().into_owned();
            return Ok(ToolOutput::new(json!({ "path": path_out, "applied": false })));
        }

        // Replace only the first occurrence.
        let updated = original.replacen(old_str, new_str, 1);
        tokio::fs::write(&path, updated.as_bytes()).await?;

        let path_out = path.to_string_lossy().into_owned();
        Ok(ToolOutput::new(json!({ "path": path_out, "applied": true })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
