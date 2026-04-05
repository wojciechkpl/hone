use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Reads the full content of a file and returns it as a string.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the entire content of a file and return it as a string."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read. Relative paths are resolved against the working directory."
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path_str = input.get_str("path").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'path'".to_string(),
        })?;

        let path = ctx.working_dir.join(path_str);
        let content = tokio::fs::read_to_string(&path).await?;

        Ok(ToolOutput::new(json!({ "content": content })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
