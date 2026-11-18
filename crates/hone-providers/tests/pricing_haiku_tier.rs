#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier in the built-in table (spec H4) and user entries replacing it
//! whole (H8, table half).
//!
//! Source: https://platform.claude.com/docs/en/about-claude/pricing (2026-11-17): prompts over
//! 100,000 tokens (input + cache reads + cache writes) pay 0.50 / 2.50 / 0.05 / 0.625.

use hone_proto::{LongPromptRates, ModelId, PricingEntry};
use hone_providers::PricingTable;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn expected_tier() -> LongPromptRates {
    LongPromptRates {
        above_prompt_tokens: 100_000,
        input_per_million: 0.50,
        output_per_million: 2.50,
        cache_read_per_million: Some(0.05),
        cache_creation_per_million: Some(0.625),
    }
}

fn assert_tier(t: &LongPromptRates, what: &str) {
    let e = expected_tier();
    assert_eq!(t.above_prompt_tokens, e.above_prompt_tokens, "{what}");
    assert!(
        close(t.input_per_million, e.input_per_million),
        "{what}: input {}",
        t.input_per_million
    );
    assert!(
        close(t.output_per_million, e.output_per_million),
        "{what}: output {}",
        t.output_per_million
    );
    assert!(
        matches!(t.cache_read_per_million, Some(v) if close(v, 0.05)),
        "{what}: cache_read {:?}",
        t.cache_read_per_million
    );
    assert!(
        matches!(t.cache_creation_per_million, Some(v) if close(v, 0.625)),
        "{what}: cache_creation {:?}",
        t.cache_creation_per_million
    );
}

fn entry(table: &PricingTable, id: &str) -> PricingEntry {
    table
        .get(&ModelId::new(id))
        .unwrap_or_else(|| panic!("{id} has no price"))
        .clone()
}

#[test]
fn test_h4_anthropic_haiku_5_5_should_carry_the_long_prompt_tier() {
    let e = entry(&PricingTable::default_table(), "anthropic:claude-haiku-5-5");
    assert_tier(
        e.long_prompt
            .as_ref()
            .expect("haiku 5.5 has a long-prompt tier"),
        "anthropic:claude-haiku-5-5",
    );
}

#[test]
fn test_h4_haiku_5_5_base_rates_should_stay_at_the_standard_row() {
    let e = entry(&PricingTable::default_table(), "anthropic:claude-haiku-5-5");
    assert!(close(e.input_per_million, 0.10));
    assert!(close(e.output_per_million, 0.50));
    assert!(matches!(e.cache_read_per_million, Some(v) if close(v, 0.01)));
    assert!(matches!(e.cache_creation_per_million, Some(v) if close(v, 0.125)));
}

#[test]
fn test_h4_dated_haiku_5_5_ids_should_resolve_to_the_entry_with_its_tier() {
    let table = PricingTable::default_table();
    let base = entry(&table, "anthropic:claude-haiku-5-5");
    for id in [
        "anthropic:claude-haiku-5-5-20261001",
        "anthropic:claude-haiku-5-5-2026-10-01",
    ] {
        let e = entry(&table, id);
        assert_eq!(e.long_prompt, base.long_prompt, "{id}");
        assert!(e.long_prompt.is_some(), "{id} lost the tier");
    }
}

#[test]
fn test_h4_the_claude_cli_haiku_5_5_entry_should_keep_base_rates_and_have_no_tier() {
    // The CLI reports one usage total per turn over many requests; a tier on that total
    // would overcharge most tool-using turns.
    let e = entry(
        &PricingTable::default_table(),
        "claude-cli:claude-haiku-5-5",
    );
    assert_eq!(e.long_prompt, None, "claude-cli must not carry a tier");
    assert!(close(e.input_per_million, 0.10));
    assert!(close(e.output_per_million, 0.50));
    assert!(matches!(e.cache_read_per_million, Some(v) if close(v, 0.01)));
    assert!(matches!(e.cache_creation_per_million, Some(v) if close(v, 0.125)));
}

#[test]
fn test_h4_no_other_built_in_entry_should_have_a_tier() {
    let table = PricingTable::default_table();
    let claude = [
        "claude-fable-5-2",
        "claude-mythos-5-2",
        "claude-fable-5-1",
        "claude-mythos-5-1",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-5-5",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-sonnet-5-5",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5",
        "claude-haiku-4-5",
    ];
    let mut ids: Vec<String> = Vec::new();
    for prefix in ["anthropic", "claude-cli"] {
        ids.extend(claude.iter().map(|c| format!("{prefix}:{c}")));
    }
    ids.extend(
        [
            "anthropic:claude-haiku-3-5",
            "gemini-cli:gemini-2.5-pro",
            "gemini-cli:gemini-2.5-flash",
            "openai:gpt-4o",
            "openai:gpt-4o-mini",
        ]
        .map(String::from),
    );
    for id in ids {
        let e = entry(&table, &id);
        assert_eq!(e.long_prompt, None, "{id} must not have a tier");
    }
}

#[test]
fn test_h4_the_built_in_haiku_5_5_entry_should_price_the_worked_example_by_prompt_length() {
    // 90,000 input + 20,000 cache read + 1,000 output, as one request: prompt 110,000 -> long.
    let e = entry(&PricingTable::default_table(), "anthropic:claude-haiku-5-5");
    let long = e.calculate_cost(90_000, 1_000, 20_000, 0);
    assert!((long - 0.0485).abs() < 1e-12, "{long}");
    // 80,000 + 20,000 = exactly 100,000 -> base.
    let base = e.calculate_cost(80_000, 1_000, 20_000, 0);
    assert!((base - 0.0087).abs() < 1e-12, "{base}");
}

#[test]
fn test_h8_a_user_entry_inserted_over_haiku_5_5_should_replace_it_whole_and_drop_the_tier() {
    let mut table = PricingTable::default_table();
    table.insert(PricingEntry {
        model: ModelId::new("anthropic:claude-haiku-5-5"),
        input_per_million: 0.30,
        output_per_million: 1.50,
        cache_read_per_million: None,
        cache_creation_per_million: None,
        long_prompt: None,
    });
    let e = entry(&table, "anthropic:claude-haiku-5-5");
    assert_eq!(e.long_prompt, None);
    // A 1M-token prompt still pays the user's flat rate.
    assert!(close(e.calculate_cost(1_000_000, 0, 0, 0), 0.30));
}
