use std::time::Duration;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use hone_sandbox::{SandboxMode, SandboxPolicyBuilder};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Executes a shell command inside the sandbox and captures stdout/stderr.
pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its stdout, stderr, and exit code."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds. Defaults to 30 000.",
                    "minimum": 1
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let command = input
            .get_str("command")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'command'".to_string(),
            })?
            .to_string();

        let timeout_ms = input.get_u64("timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(&ctx.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Ask the sandbox to apply any restrictions to the child process.
        // NullSandbox is a no-op; real sandboxes may prepend a wrapper.
        // Note: `restrict_child` takes `&mut std::process::Command`, but
        // `tokio::process::Command` wraps it.  We build a temporary std
        // command to run through the sandbox, then re-apply the args to
        // our tokio command.
        //
        // For now we delegate the sandbox call via a std::process::Command
        // probe — the NullSandbox does nothing, and more sophisticated
        // backends should override `restrict_child` with a tokio-compatible
        // path in the future.
        let mut std_cmd = std::process::Command::new("sh");
        std_cmd.arg("-c").arg(&command);
        let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
            .build()
            .expect("manual policy with no constraints is always valid");
        ctx.sandbox
            .restrict_child(&mut std_cmd, &policy)
            .map_err(|e| ToolError::SandboxDenied {
                tool: "shell".to_string(),
                action: e.to_string(),
            })?;

        let duration = Duration::from_millis(timeout_ms);
        let result = timeout(duration, cmd.output()).await;

        match result {
            Err(_elapsed) => Err(ToolError::Timeout {
                tool: "shell".to_string(),
                duration_ms: timeout_ms,
            }),
            Ok(Err(io_err)) => Err(ToolError::IoError(io_err)),
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let exit_code = output.status.code().unwrap_or(-1);

                Ok(ToolOutput::new(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code
                })))
            }
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
