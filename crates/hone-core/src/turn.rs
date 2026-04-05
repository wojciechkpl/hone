use hone_proto::TokenUsage;
use serde::{Deserialize, Serialize};

/// The result of a single agent turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnResult {
    /// The final text response from the assistant.
    pub response: String,
    /// Token usage for this turn.
    pub usage: TokenUsage,
    /// Cost in USD for this turn.
    pub cost_usd: f64,
    /// Number of tool calls made during this turn.
    pub tool_calls_count: u32,
}
