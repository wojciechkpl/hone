use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Reverts the last commit by running `git revert HEAD --no-edit`.
pub struct GitUndoTool;

#[async_trait]
impl Tool for GitUndoTool {
    fn name(&self) -> &str {
        "git_undo"
    }

    fn description(&self) -> &str {
        "Revert the last git commit by creating a new revert commit. \
         Returns the SHA of the revert commit and its message."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let working_dir = ctx.working_dir.clone();

        let revert_out = Command::new("git")
            .args(["revert", "HEAD", "--no-edit"])
            .current_dir(&working_dir)
            .output()
            .await?;

        if !revert_out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "git_undo".to_string(),
                message: String::from_utf8_lossy(&revert_out.stderr).to_string(),
            });
        }

        // Retrieve the SHA of the revert commit.
        let rev_out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&working_dir)
            .output()
            .await?;

        if !rev_out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "git_undo".to_string(),
                message: String::from_utf8_lossy(&rev_out.stderr).to_string(),
            });
        }

        let sha = String::from_utf8_lossy(&rev_out.stdout).trim().to_string();

        // Retrieve the commit message of the revert commit.
        let msg_out = Command::new("git")
            .args(["log", "-1", "--pretty=%s", "HEAD"])
            .current_dir(&working_dir)
            .output()
            .await?;

        let message = if msg_out.status.success() {
            String::from_utf8_lossy(&msg_out.stdout).trim().to_string()
        } else {
            String::new()
        };

        Ok(ToolOutput::new(json!({
            "reverted_sha": sha,
            "message": message,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
