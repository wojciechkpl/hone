use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Creates and/or switches to a git branch.
pub struct GitBranchTool;

#[async_trait]
impl Tool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn description(&self) -> &str {
        "Create and/or switch to a git branch. \
         Set 'create' to true to create the branch before switching."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The branch name to create or switch to."
                },
                "create": {
                    "type": "boolean",
                    "description": "If true, create the branch before switching."
                }
            },
            "required": ["name"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let name = input
            .get_str("name")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'name'".to_string(),
            })?
            .to_string();

        let create = input.get_bool("create").unwrap_or(false);
        let working_dir = ctx.working_dir.clone();

        // Build the checkout command: `-b` to create, or just switch if it already exists.
        let mut args = vec!["checkout".to_string()];
        if create {
            args.push("-b".to_string());
        }
        args.push(name.clone());

        let out = Command::new("git")
            .args(&args)
            .current_dir(&working_dir)
            .output()
            .await?;

        if !out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "git_branch".to_string(),
                message: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }

        // Retrieve the current HEAD SHA.
        let rev_out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&working_dir)
            .output()
            .await?;

        let head = if rev_out.status.success() {
            String::from_utf8_lossy(&rev_out.stdout).trim().to_string()
        } else {
            String::new()
        };

        Ok(ToolOutput::new(json!({
            "branch": name,
            "head": head,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
