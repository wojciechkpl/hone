use serde::{Deserialize, Serialize};

use crate::model::ModelId;

/// A cost estimate for an upcoming operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub model: ModelId,
    pub estimated_input_tokens: u32,
    pub estimated_output_tokens: u32,
    pub estimated_cost_usd: f64,
    pub session_total_usd: f64,
    pub budget_remaining_usd: Option<f64>,
}

/// Pricing entry for a specific model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingEntry {
    pub model: ModelId,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
}

impl PricingEntry {
    /// Calculate cost in USD for given token counts.
    pub fn calculate_cost(&self, input_tokens: u32, output_tokens: u32, cache_tokens: u32) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_per_million;
        let cache_cost = self
            .cache_read_per_million
            .map(|rate| (cache_tokens as f64 / 1_000_000.0) * rate)
            .unwrap_or(0.0);
        input_cost + output_cost + cache_cost
    }
}
