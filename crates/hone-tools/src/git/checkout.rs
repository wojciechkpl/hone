use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Switches to an existing git branch.
pub struct GitCheckoutTool;

#[async_trait]
impl Tool for GitCheckoutTool {
    fn name(&self) -> &str {
        "git_checkout"
    }

    fn description(&self) -> &str {
        "Switch to an existing git branch. \
         Returns the branch name and the HEAD SHA after checkout."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "branch": {
                    "type": "string",
                    "description": "The name of the existing branch to switch to."
                }
            },
            "required": ["branch"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let branch = input
            .get_str("branch")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'branch'".to_string(),
            })?
            .to_string();

        let working_dir = ctx.working_dir.clone();

        let out = Command::new("git")
            .args(["checkout", &branch])
            .current_dir(&working_dir)
            .output()
            .await?;

        if !out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "git_checkout".to_string(),
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
            "branch": branch,
            "head": head,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
