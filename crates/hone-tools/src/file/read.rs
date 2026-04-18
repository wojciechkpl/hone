use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Reads file contents with optional line range, returning line-numbered output.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read file contents with optional line range. Returns line-numbered content \
         (1-indexed) and metadata including total line count.\n\n\
         WHEN TO USE: To understand existing code before editing. To verify edits after writing. \
         To check file structure and find specific lines.\n\
         WHEN NOT TO USE: If you only need to find which files contain a pattern — use grep instead. \
         If you need the full directory listing — use glob instead.\n\
         PREREQUISITE: File must exist. Use glob first if unsure of the exact path.\n\
         Use offset/limit for large files to avoid reading the whole file at once. Default limit: 500 lines."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file. Relative paths are resolved against the working directory."
                },
                "offset": {
                    "type": "integer",
                    "description": "1-indexed line number to start reading from. Default: 1.",
                    "minimum": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to return. Default: 500. Files longer than limit are truncated.",
                    "minimum": 1
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let path_str = input.get_str("path").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'path'".to_string(),
        })?;

        let offset = input.get_u64("offset").unwrap_or(1).max(1) as usize;
        let limit = input.get_u64("limit").unwrap_or(500) as usize;

        let path = resolve_within_working_dir(&ctx.working_dir, path_str)?;
        let full_content = tokio::fs::read_to_string(&path).await?;

        let all_lines: Vec<&str> = full_content.lines().collect();
        let total_lines = all_lines.len();

        let start_idx = offset.saturating_sub(1); // convert to 0-indexed
        let end_idx = (start_idx + limit).min(total_lines);
        let truncated = end_idx < total_lines;

        // Format with line numbers: "   42 | content"
        let max_line_width = if end_idx > 0 {
            end_idx.to_string().len().max(4)
        } else {
            4
        };

        let mut formatted = String::new();
        for (i, line) in all_lines[start_idx..end_idx].iter().enumerate() {
            let line_num = start_idx + i + 1;
            formatted.push_str(&format!(
                "{:>width$} | {}\n",
                line_num,
                line,
                width = max_line_width
            ));
        }

        Ok(ToolOutput::new(json!({
            "content": formatted,
            "start_line": start_idx + 1,
            "end_line": end_idx,
            "total_lines": total_lines,
            "truncated": truncated,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
