use std::path::PathBuf;
use std::sync::Arc;

use hone_sandbox::{Sandbox, SandboxMode, SandboxPolicyBuilder};
use tokio::process::Command;

use crate::AgentError;

/// Runner that executes a script inside a sandbox and captures its output.
pub struct CodeModeRunner {
    sandbox: Arc<dyn Sandbox>,
    working_dir: PathBuf,
}

/// Output produced by a sandboxed script execution.
#[derive(Debug)]
pub struct CodeModeOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CodeModeRunner {
    pub fn new(sandbox: Arc<dyn Sandbox>, working_dir: PathBuf) -> Self {
        Self {
            sandbox,
            working_dir,
        }
    }

    /// Write `script` to a temporary file and execute it with `interpreter`.
    ///
    /// The temporary file is removed automatically when it goes out of scope.
    /// `restrict_child` is called on the spawned [`std::process::Command`]
    /// before it is converted to a [`tokio::process::Command`] so that the
    /// sandbox policy is applied to the child process.
    pub async fn execute(
        &self,
        script: &str,
        interpreter: &str,
    ) -> Result<CodeModeOutput, AgentError> {
        let tmp = tempfile::NamedTempFile::new().map_err(|e| AgentError::CodeMode {
            detail: format!("failed to create tempfile: {e}"),
        })?;

        tokio::fs::write(tmp.path(), script)
            .await
            .map_err(|e| AgentError::CodeMode {
                detail: format!("failed to write script to tempfile: {e}"),
            })?;

        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(self.working_dir.clone())
            .allow_write(self.working_dir.clone())
            .build()
            .map_err(|e| AgentError::CodeMode {
                detail: format!("failed to build sandbox policy: {e}"),
            })?;

        let mut std_cmd = std::process::Command::new(interpreter);
        std_cmd.arg(tmp.path()).current_dir(&self.working_dir);

        self.sandbox
            .restrict_child(&mut std_cmd, &policy)
            .map_err(|e| AgentError::CodeMode {
                detail: format!("sandbox restrict_child failed: {e}"),
            })?;

        let mut cmd = Command::from(std_cmd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = cmd.output().await.map_err(|e| AgentError::CodeMode {
            detail: format!("failed to spawn interpreter '{interpreter}': {e}"),
        })?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        Ok(CodeModeOutput {
            stdout,
            stderr,
            exit_code,
        })
    }
}
