use hone_providers::ProviderError;
use hone_tools::ToolError;
use thiserror::Error;

/// Errors that can occur during agent operation.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("tool '{name}' failed: {source}")]
    Tool { name: String, source: ToolError },

    #[error("tool '{name}' not found in registry")]
    ToolNotFound { name: String },

    #[error("budget exceeded: limit ${limit_usd:.4}, current ${current_usd:.4}")]
    BudgetExceeded { limit_usd: f64, current_usd: f64 },

    #[error("agent was cancelled")]
    Cancelled,

    #[error("context overflow: max {max_tokens} tokens")]
    ContextOverflow { max_tokens: u32 },

    #[error("code mode error: {detail}")]
    CodeMode { detail: String },

    #[error("internal error: {detail}")]
    Internal { detail: String },
}
