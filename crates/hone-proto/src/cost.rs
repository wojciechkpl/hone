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
    /// Cost per million tokens served from the prompt cache (typically 0.1× input rate).
    pub cache_read_per_million: Option<f64>,
    /// Cost per million tokens written into the prompt cache (typically 1.25× input rate).
    /// Derived automatically as `1.25 * input_per_million` when `None`.
    #[serde(default)]
    pub cache_creation_per_million: Option<f64>,
}

impl PricingEntry {
    /// Calculate cost in USD for given token counts.
    pub fn calculate_cost(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_creation_tokens: u32,
    ) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_per_million;
        let cache_read_cost = self
            .cache_read_per_million
            .map(|rate| (cache_read_tokens as f64 / 1_000_000.0) * rate)
            .unwrap_or(0.0);
        // Cache creation is billed at 1.25× input rate when no explicit rate is set.
        let creation_rate = self
            .cache_creation_per_million
            .unwrap_or(self.input_per_million * 1.25);
        let cache_creation_cost = (cache_creation_tokens as f64 / 1_000_000.0) * creation_rate;
        input_cost + output_cost + cache_read_cost + cache_creation_cost
    }
}
