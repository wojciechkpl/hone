use async_trait::async_trait;
use hone_proto::ToolLayer;
use ignore::WalkBuilder;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Walks the filesystem and returns paths that match a glob pattern.
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Uses `.gitignore` rules by default. \
         The optional `path` parameter restricts the search root.\n\n\
         WHEN TO USE: To discover what files exist in the project. To find files by extension \
         (e.g. '**/*.rs', 'src/**/*.py'). To understand project structure before diving in.\n\
         WHEN NOT TO USE: If you need to search file CONTENTS — use grep instead. \
         If you already know the exact file path — use read_file directly."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match against file paths (e.g. '**/*.rs')."
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory to search within. Defaults to the working directory."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let pattern = input
            .get_str("pattern")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'pattern'".to_string(),
            })?
            .to_string();

        let search_root = match input.get_str("path") {
            Some(p) => resolve_within_working_dir(&ctx.working_dir, p)?,
            None => ctx.working_dir.clone(),
        };

        // Build a glob matcher using the `globset` crate (a direct dependency of `ignore`).
        let glob_matcher = {
            use globset::{Glob, GlobSetBuilder};
            let mut builder = GlobSetBuilder::new();
            builder.add(Glob::new(&pattern).map_err(|e| ToolError::InvalidInput {
                reason: format!("invalid glob pattern: {e}"),
            })?);
            builder.build().map_err(|e| ToolError::InvalidInput {
                reason: format!("failed to build glob set: {e}"),
            })?
        };

        // Run the directory walk on a blocking thread to avoid blocking the async runtime.
        let files: Vec<String> = tokio::task::spawn_blocking(move || {
            let mut matches = Vec::new();
            for entry in WalkBuilder::new(&search_root).build().flatten() {
                let path = entry.path();
                if path.is_file() {
                    // Match against the path relative to the search root if possible,
                    // falling back to the full path.
                    let rel = path.strip_prefix(&search_root).unwrap_or(path);

                    if glob_matcher.is_match(rel) {
                        matches.push(path.to_string_lossy().into_owned());
                    }
                }
            }
            matches
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "glob".to_string(),
            message: e.to_string(),
        })?;

        Ok(ToolOutput::new(json!({ "files": files })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
