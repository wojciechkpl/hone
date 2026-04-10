use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Performs an exact string replacement in a file, enforcing uniqueness of `old_str`.
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Replace old_str with new_str in the specified file. old_str must appear EXACTLY ONCE \
         in the file — if it appears multiple times, include more surrounding context to make it unique. \
         Returns applied: false when old_str is not found; errors when old_str is ambiguous.\n\n\
         WHEN TO USE: To make targeted changes to existing files. Preferred over write_file for modifications.\n\
         WHEN NOT TO USE: To create new files (use write_file). To make many changes at once (use multi_edit).\n\
         PREREQUISITE: Always read_file first to get the exact content for old_str. \
         Copy the text exactly as it appears — whitespace and indentation matter."
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
                    "description": "The exact string to search for. Must appear exactly once in the file."
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

        let path = resolve_within_working_dir(&ctx.working_dir, path_str)?;
        let original = tokio::fs::read_to_string(&path).await?;

        let count = original.matches(old_str).count();

        if count == 0 {
            let path_out = path.to_string_lossy().into_owned();
            return Ok(ToolOutput::new(json!({
                "path": path_out,
                "applied": false,
                "reason": "old_str not found in file"
            })));
        }

        if count > 1 {
            // Collect line numbers of all matches (cap at 10 for readability).
            let mut line_numbers: Vec<usize> = Vec::new();
            for (line_num, line) in original.lines().enumerate() {
                if line.contains(old_str) {
                    line_numbers.push(line_num + 1);
                    if line_numbers.len() >= 10 {
                        break;
                    }
                }
            }

            return Err(ToolError::InvalidInput {
                reason: format!(
                    "old_str appears {} times in {} (lines: {:?}). Include more surrounding context to make it unique.",
                    count,
                    path.display(),
                    line_numbers,
                ),
            });
        }

        // Exactly one match — safe to apply.
        let updated = original.replacen(old_str, new_str, 1);
        tokio::fs::write(&path, updated.as_bytes()).await?;

        // Read back and verify the written content matches what was intended.
        let verification = tokio::fs::read_to_string(&path).await.map_err(|e| {
            ToolError::Other(format!("failed to verify write: {e}"))
        })?;
        if verification != updated {
            return Err(ToolError::Other(format!(
                "write verification failed: content written to {} does not match expected",
                path.display()
            )));
        }

        let path_out = path.to_string_lossy().into_owned();
        Ok(ToolOutput::new(json!({ "path": path_out, "applied": true })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
