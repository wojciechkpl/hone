use hone_proto::{PricingEntry, TokenUsage};

use crate::error::AgentError;

/// Accumulates token usage and USD cost across multiple turns.
#[derive(Debug, Default)]
pub struct CostAccumulator {
    total_usage: TokenUsage,
    total_usd: f64,
}

impl CostAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record usage from a completed turn, converting tokens to USD via pricing.
    pub fn add(&mut self, usage: TokenUsage, pricing: &PricingEntry) {
        let cost = pricing.calculate_cost(
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_creation,
        );
        self.total_usd += cost;
        self.total_usage = self.total_usage + usage;
    }

    /// Total accumulated cost in USD.
    pub fn total_usd(&self) -> f64 {
        self.total_usd
    }

    /// Total accumulated token usage.
    pub fn total_usage(&self) -> TokenUsage {
        self.total_usage
    }

    /// Returns `BudgetExceeded` if `limit` is set and has been surpassed.
    pub fn check_budget(&self, limit: Option<f64>) -> Result<(), AgentError> {
        if let Some(limit_usd) = limit {
            if self.total_usd > limit_usd {
                return Err(AgentError::BudgetExceeded {
                    limit_usd,
                    current_usd: self.total_usd,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::{ModelId, PricingEntry, TokenUsage};

    fn pricing() -> PricingEntry {
        PricingEntry {
            model: ModelId::new("test:model"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        }
    }

    #[test]
    fn test_cost_accumulator_sums_turns() {
        let mut acc = CostAccumulator::new();
        let p = pricing();

        // 1M input tokens = $3, 1M output = $15 → $18 each turn
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 0,
            cache_creation: 0,
        };

        acc.add(usage, &p);
        acc.add(usage, &p);
        acc.add(usage, &p);

        assert!((acc.total_usd() - 54.0).abs() < 1e-9);
        assert_eq!(acc.total_usage().input, 3_000_000);
        assert_eq!(acc.total_usage().output, 3_000_000);
    }

    #[test]
    fn test_budget_exceeded() {
        let mut acc = CostAccumulator::new();
        let p = pricing();

        // Add usage that costs $0.03
        acc.add(
            TokenUsage {
                input: 10_000,
                output: 0,
                cache_read: 0,
                cache_creation: 0,
            },
            &p,
        );

        // Budget of $0.01 should trigger error
        let result = acc.check_budget(Some(0.01));
        assert!(matches!(result, Err(AgentError::BudgetExceeded { .. })));
    }

    #[test]
    fn test_budget_none_never_exceeds() {
        let mut acc = CostAccumulator::new();
        let p = pricing();

        acc.add(
            TokenUsage {
                input: 1_000_000,
                output: 1_000_000,
                cache_read: 0,
                cache_creation: 0,
            },
            &p,
        );

        // None budget never triggers
        assert!(acc.check_budget(None).is_ok());
    }
}
