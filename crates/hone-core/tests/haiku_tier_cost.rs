#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier, `CostAccumulator` side (spec H5): `add` keeps one-request
//! semantics (the tier follows the usage passed in), and separate `add` calls are separate
//! requests that never share a prompt length.

use hone_core::CostAccumulator;
use hone_proto::{LongPromptRates, ModelId, PricingEntry, TokenUsage};

fn tiered() -> PricingEntry {
    PricingEntry {
        model: ModelId::new("test:tiered"),
        input_per_million: 1.0,
        output_per_million: 2.0,
        cache_read_per_million: Some(0.1),
        cache_creation_per_million: Some(1.25),
        long_prompt: Some(LongPromptRates {
            above_prompt_tokens: 100_000,
            input_per_million: 10.0,
            output_per_million: 20.0,
            cache_read_per_million: Some(1.0),
            cache_creation_per_million: Some(12.5),
        }),
    }
}

fn u(input: u32, output: u32, cache_read: u32, cache_creation: u32) -> TokenUsage {
    TokenUsage {
        input,
        output,
        cache_read,
        cache_creation,
    }
}

#[test]
fn test_h5_add_should_price_its_usage_as_one_request_with_the_tier_chosen_by_its_prompt() {
    let mut acc = CostAccumulator::new();
    acc.add(u(60_000, 1_000, 0, 50_000), &tiered()); // prompt 110,000 -> long
                                                     // 60,000 x 10 + 1,000 x 20 + 50,000 x 12.5 per million
    assert!(
        (acc.total_usd() - 1.245).abs() < 1e-12,
        "{}",
        acc.total_usd()
    );
}

#[test]
fn test_h5_separate_add_calls_should_not_share_a_prompt_length() {
    let mut acc = CostAccumulator::new();
    acc.add(u(60_000, 0, 0, 0), &tiered());
    acc.add(u(60_000, 0, 0, 0), &tiered()); // 120,000 in total, but two base-rate requests
    assert!(
        (acc.total_usd() - 0.12).abs() < 1e-12,
        "{}",
        acc.total_usd()
    );
    assert_eq!(acc.total_usage().input, 120_000);
}
