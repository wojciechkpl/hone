use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Creates or overwrites a file with the given content.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it if it does not exist or overwriting it if it does.\n\n\
         WHEN TO USE: To create new files. To completely rewrite a file with new content.\n\
         WHEN NOT TO USE: To make small changes to existing files — use edit_file instead \
         (it preserves the parts you don't change and enforces uniqueness). \
         Never use write_file to modify a file you haven't read first.\n\
         PREREQUISITE: Read the file first if it exists. Parent directories are created automatically."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file."
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path_str = input
            .get_str("path")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'path'".to_string(),
            })?;

        let content = input
            .get_str("content")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'content'".to_string(),
            })?;

        let path = resolve_within_working_dir(&ctx.working_dir, path_str)?;

        // Create parent directories if they don't exist.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let bytes = content.as_bytes();
        tokio::fs::write(&path, bytes).await?;

        // Read back and verify the written content matches what was intended.
        let verification = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Other(format!("failed to verify write: {e}")))?;
        if verification != content {
            return Err(ToolError::Other(format!(
                "write verification failed: content written to {} does not match expected",
                path.display()
            )));
        }

        let path_out = path.to_string_lossy().into_owned();
        Ok(ToolOutput::new(
            json!({ "path": path_out, "bytes_written": bytes.len() }),
        ))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
