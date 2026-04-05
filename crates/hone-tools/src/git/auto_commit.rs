use std::path::Path;

use tokio::process::Command;

use crate::error::ToolError;

/// Wraps a write operation with automatic git stage + commit.
pub struct AutoCommitter;

impl AutoCommitter {
    /// Stage `file_path` and create a commit with `message` inside `working_dir`.
    /// Returns the full SHA of the new commit.
    pub async fn commit_file(
        working_dir: &Path,
        file_path: &Path,
        message: &str,
    ) -> Result<String, ToolError> {
        // Stage the file.
        let add_out = Command::new("git")
            .args(["add", "--"])
            .arg(file_path)
            .current_dir(working_dir)
            .output()
            .await?;

        if !add_out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "auto_commit".to_string(),
                message: String::from_utf8_lossy(&add_out.stderr).to_string(),
            });
        }

        // Create the commit.
        let commit_out = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(working_dir)
            .output()
            .await?;

        if !commit_out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "auto_commit".to_string(),
                message: String::from_utf8_lossy(&commit_out.stderr).to_string(),
            });
        }

        // Retrieve the SHA of the new HEAD.
        let rev_out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(working_dir)
            .output()
            .await?;

        if !rev_out.status.success() {
            return Err(ToolError::ExecutionFailed {
                tool: "auto_commit".to_string(),
                message: String::from_utf8_lossy(&rev_out.stderr).to_string(),
            });
        }

        let sha = String::from_utf8_lossy(&rev_out.stdout).trim().to_string();
        Ok(sha)
    }
}
