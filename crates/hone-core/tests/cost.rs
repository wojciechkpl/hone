use hone_core::{AgentError, CostAccumulator};
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

    // 1M input = $3, 1M output = $15 → $18 per turn
    let usage = TokenUsage {
        input: 1_000_000,
        output: 1_000_000,
        cache_read: 0,
        cache_creation: 0,
    };

    acc.add(usage, &p);
    acc.add(usage, &p);
    acc.add(usage, &p);

    assert!(
        (acc.total_usd() - 54.0).abs() < 1e-9,
        "expected ~$54, got {}",
        acc.total_usd()
    );
    assert_eq!(acc.total_usage().input, 3_000_000);
    assert_eq!(acc.total_usage().output, 3_000_000);
}

#[test]
fn test_budget_exceeded() {
    let mut acc = CostAccumulator::new();
    let p = pricing();

    // 10k input tokens at $3/M = $0.03
    acc.add(
        TokenUsage {
            input: 10_000,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        },
        &p,
    );

    let result = acc.check_budget(Some(0.01));
    assert!(
        matches!(result, Err(AgentError::BudgetExceeded { limit_usd, current_usd })
            if limit_usd == 0.01 && current_usd > 0.01),
        "expected BudgetExceeded error"
    );
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

    assert!(
        acc.check_budget(None).is_ok(),
        "None budget should never trigger BudgetExceeded"
    );
}
