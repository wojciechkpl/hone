pub mod validation;

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
const MAX_TIMEOUT_MS: u64 = 300_000;

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

        // Validate the command before executing.
        match validation::validate_command(&command) {
            validation::ValidationResult::Allow => {}
            validation::ValidationResult::Block(reason) => {
                return Err(ToolError::InvalidInput {
                    reason: format!("command blocked: {reason}"),
                });
            }
            validation::ValidationResult::RequireApproval(reason) => {
                // For v1, log a warning and proceed. A future version can gate
                // on an approval mechanism.
                tracing::warn!(
                    command = %command,
                    reason = %reason,
                    "command requires approval but v1 policy auto-approves with warning"
                );
            }
        }

        let timeout_ms = input
            .get_u64("timeout_ms")
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // Build a std::process::Command first so we can pass it through
        // restrict_child (which takes &mut std::process::Command).  The
        // sandbox may rewrite the command entirely (e.g. prepend sandbox-exec),
        // so we capture the final program and args afterwards and reconstruct a
        // tokio::process::Command from them.
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

        // Reconstruct a tokio Command from the (possibly rewritten) std Command.
        // We must read program/args before converting because Into<tokio::Command>
        // is not available — tokio::process::Command has its own constructor.
        let program = std_cmd.get_program().to_os_string();
        let args: Vec<std::ffi::OsString> =
            std_cmd.get_args().map(|a| a.to_os_string()).collect();

        let mut cmd = Command::new(program);
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.current_dir(&ctx.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

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
