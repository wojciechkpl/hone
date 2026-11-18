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

/// Rates that replace a [`PricingEntry`]'s base rates for requests whose prompt is long.
///
/// A request's prompt length is its input, cache-read and cache-write tokens together. A
/// request over `above_prompt_tokens` pays these rates for all four token categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongPromptRates {
    /// Prompt length above which these rates apply (exactly this length is still base).
    pub above_prompt_tokens: u64,
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
    /// Derived as `1.25 * input_per_million` when `None`.
    #[serde(default)]
    pub cache_creation_per_million: Option<f64>,
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
    /// Higher rates for requests with a long prompt, when the model has such a tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_prompt: Option<LongPromptRates>,
}

impl PricingEntry {
    /// Calculate cost in USD for the token counts of **one request**; the prompt length
    /// (input + cache read + cache write) selects the tier.
    pub fn calculate_cost(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_creation_tokens: u32,
    ) -> f64 {
        let prompt = input_tokens as u64 + cache_read_tokens as u64 + cache_creation_tokens as u64;
        self.calculate_cost_with_prompt(
            prompt,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        )
    }

    /// Like [`calculate_cost`](Self::calculate_cost), but the tier is chosen by
    /// `prompt_tokens` (the request's known prompt length), not by the arguments. Prices one
    /// usage event of a request, e.g. an output-only event.
    pub fn calculate_cost_with_prompt(
        &self,
        prompt_tokens: u64,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_creation_tokens: u32,
    ) -> f64 {
        let (input_rate, output_rate, read_rate, creation_rate) = match &self.long_prompt {
            Some(t) if prompt_tokens > t.above_prompt_tokens => (
                t.input_per_million,
                t.output_per_million,
                t.cache_read_per_million,
                t.cache_creation_per_million,
            ),
            _ => (
                self.input_per_million,
                self.output_per_million,
                self.cache_read_per_million,
                self.cache_creation_per_million,
            ),
        };
        let input_cost = (input_tokens as f64 / 1_000_000.0) * input_rate;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * output_rate;
        let cache_read_cost = read_rate
            .map(|rate| (cache_read_tokens as f64 / 1_000_000.0) * rate)
            .unwrap_or(0.0);
        // Cache creation is billed at 1.25× input rate when no explicit rate is set.
        let creation_rate = creation_rate.unwrap_or(input_rate * 1.25);
        let cache_creation_cost = (cache_creation_tokens as f64 / 1_000_000.0) * creation_rate;
        input_cost + output_cost + cache_read_cost + cache_creation_cost
    }
}
