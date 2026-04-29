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
        "Run a shell command and return its stdout, stderr, and exit code.\n\n\
         WHEN TO USE: To run tests (cargo test, pytest, npm test). To check build status. \
         To run linters or formatters. To execute git commands not covered by git_* tools.\n\
         WHEN NOT TO USE: To read files — use read_file (faster, line-numbered). \
         To search files — use grep (faster, respects gitignore). \
         To list files — use glob (structured output).\n\
         COST: Spawns a subprocess. Use specific tools when available — they are faster and safer.\n\
         SAFETY: Destructive commands (rm -rf, sudo) and network commands (curl, wget) are blocked. \
         Compound operators (;, &&, ||, |) are treated as destructive."
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
                return Err(ToolError::InvalidInput {
                    reason: format!("command blocked (requires approval): {reason}"),
                });
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

        // Build a concrete policy anchored to the working directory.
        //
        // Minimum surfaces:
        //   - working_dir: read + write (the agent's project root)
        //   - /tmp (/private/tmp on macOS): writable temp space
        //   - /bin, /usr/bin: readable so sh, cat, ls etc. can exec
        //   - /etc: readable for config files (ssl certs, resolv.conf, …)
        //   - $HOME/.cache, $HOME/.cargo: readable for toolchain caches,
        //     only when not already under working_dir
        //
        // SandboxMode::Autonomous requires at least one readable path and is
        // the most restrictive mode available — use it so the builder rejects
        // a misconfigured policy at build time.
        let mut builder = SandboxPolicyBuilder::new(SandboxMode::Autonomous)
            .allow_write(ctx.working_dir.clone()) // implies read
            .allow_write(std::path::PathBuf::from("/tmp"))
            .allow_read(std::path::PathBuf::from("/bin"))
            .allow_read(std::path::PathBuf::from("/usr/bin"))
            .allow_read(std::path::PathBuf::from("/usr/lib"))
            .allow_read(std::path::PathBuf::from("/usr/share"))
            .allow_read(std::path::PathBuf::from("/etc"));

        // Allow user cache dirs only when outside working_dir, so that
        // toolchain tools (cargo, pip …) can read their caches.
        if let Ok(home) = std::env::var("HOME") {
            let home_path = std::path::PathBuf::from(&home);
            for cache_rel in [".cache", ".cargo", ".rustup"] {
                let cache_path = home_path.join(cache_rel);
                if !cache_path.starts_with(&ctx.working_dir) {
                    builder = builder.allow_read(cache_path);
                }
            }
        }

        let policy = builder.build().map_err(|e| ToolError::SandboxDenied {
            tool: "shell".to_string(),
            action: format!("policy build failed: {e}"),
        })?;

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
        let args: Vec<std::ffi::OsString> = std_cmd.get_args().map(|a| a.to_os_string()).collect();

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
