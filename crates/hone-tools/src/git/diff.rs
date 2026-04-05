use async_trait::async_trait;
use git2::{DiffOptions, Repository};
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Returns a unified diff of working-tree or staged changes.
pub struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Return the git diff for the repository. \
         Pass `staged: true` to diff the index against HEAD instead of the working tree."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "staged": {
                    "type": "boolean",
                    "description": "If true, show staged (index) changes instead of unstaged working-tree changes."
                }
            }
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let staged = input.get_bool("staged").unwrap_or(false);
        let working_dir = ctx.working_dir.clone();

        let result = tokio::task::spawn_blocking(move || {
            let repo = Repository::discover(&working_dir)?;
            let mut opts = DiffOptions::new();

            let diff = if staged {
                // Diff HEAD tree vs index (staged changes).
                let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                repo.diff_tree_to_index(head.as_ref(), None, Some(&mut opts))?
            } else {
                // Diff index vs working tree (unstaged changes).
                repo.diff_index_to_workdir(None, Some(&mut opts))?
            };

            let mut patch = String::new();
            diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
                use std::str;
                let origin = line.origin();
                if origin == '+' || origin == '-' || origin == ' ' {
                    patch.push(origin);
                }
                if let Ok(s) = str::from_utf8(line.content()) {
                    patch.push_str(s);
                }
                true
            })?;

            Ok::<_, git2::Error>(patch)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "git_diff".to_string(),
            message: e.to_string(),
        })?
        .map_err(ToolError::from)?;

        Ok(ToolOutput::new(json!({ "diff": result })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
