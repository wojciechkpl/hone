use thiserror::Error;

/// Errors that can occur during tool execution.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("tool '{tool}' timed out after {duration_ms}ms")]
    Timeout { tool: String, duration_ms: u64 },

    #[error("sandbox denied action '{action}' for tool '{tool}'")]
    SandboxDenied { tool: String, action: String },

    #[error("tool '{tool}' execution failed: {message}")]
    ExecutionFailed { tool: String, message: String },

    #[error("git error: {0}")]
    GitError(String),

    #[error("{0}")]
    Other(String),
}

impl From<git2::Error> for ToolError {
    fn from(e: git2::Error) -> Self {
        ToolError::GitError(e.message().to_string())
    }
}
