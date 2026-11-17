#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U6 of `docs/plans/usage-and-pricing-spec.md`: a model id ending in `-YYYY-MM-DD` is priced
//! with its base id's entry when it has none of its own.

use hone_proto::{ModelId, PricingEntry};
use hone_providers::PricingTable;

fn rates(id: &str) -> Option<(f64, f64)> {
    PricingTable::default_table()
        .get(&ModelId::new(id))
        .map(|e| (e.input_per_million, e.output_per_million))
}

#[test]
fn test_u6_a_dated_id_should_use_its_base_entry() {
    let base = rates("openai:gpt-4o").expect("gpt-4o is in the table");
    assert_eq!(rates("openai:gpt-4o-2024-08-06"), Some(base));
    assert_eq!(rates("openai:gpt-4o-2024-11-20"), Some(base));
}

#[test]
fn test_u6_the_longest_base_should_win() {
    let mini = rates("openai:gpt-4o-mini").expect("gpt-4o-mini is in the table");
    assert_eq!(rates("openai:gpt-4o-mini-2024-07-18"), Some(mini));
    assert_ne!(
        rates("openai:gpt-4o-mini-2024-07-18"),
        rates("openai:gpt-4o")
    );
}

#[test]
fn test_u6_only_a_dash_separated_date_suffix_counts() {
    assert_eq!(rates("openai:gpt-4o-preview"), None);
    assert_eq!(rates("openai:gpt-4o-2024-08"), None, "incomplete date");
    assert_eq!(
        rates("openai:unknown-model-2024-08-06"),
        None,
        "the base has no entry either"
    );
}

#[test]
fn test_u6_an_exact_entry_should_still_win_over_the_base() {
    // Lookups of ids the table lists exactly are unchanged by the date-suffix fallback.
    let table = PricingTable::default_table();
    let e = table.get(&ModelId::new("openai:gpt-4o")).unwrap();
    assert_eq!(e.model.as_str(), "openai:gpt-4o");
}

// ---- U16: Anthropic-style -YYYYMMDD suffixes ----------------------------------------------------

#[test]
fn test_u16_a_trailing_yyyymmdd_should_be_stripped_to_find_the_base_entry() {
    let sonnet = rates("anthropic:claude-sonnet-4-5").expect("in the table");
    assert_eq!(rates("anthropic:claude-sonnet-4-5-20250929"), Some(sonnet));
    let opus = rates("anthropic:claude-opus-4-5").expect("in the table");
    assert_eq!(rates("anthropic:claude-opus-4-5-20251101"), Some(opus));
    let gpt = rates("openai:gpt-4o").unwrap();
    assert_eq!(
        rates("openai:gpt-4o-20240806"),
        Some(gpt),
        "the rule is the same for every provider"
    );
}

#[test]
fn test_u16_only_eight_digits_after_a_dash_count() {
    assert_eq!(rates("anthropic:claude-sonnet-4-5-2025"), None);
    assert_eq!(
        rates("anthropic:claude-sonnet-4-5-202509291"),
        None,
        "nine digits"
    );
    assert_eq!(
        rates("anthropic:unknown-model-20250929"),
        None,
        "the base has no entry either"
    );
}

#[test]
fn test_u16_a_listed_dated_entry_should_still_win() {
    // An exact -YYYYMMDD entry wins over its base; the base id keeps its own price.
    let mut table = PricingTable::default_table();
    let base = table
        .get(&ModelId::new("anthropic:claude-haiku-4-5"))
        .expect("alias is in the table")
        .input_per_million;
    let dated_price = base + 7.0;
    table.insert(PricingEntry {
        model: ModelId::new("anthropic:claude-haiku-4-5-20251001"),
        input_per_million: dated_price,
        output_per_million: 1.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    });
    let dated = table
        .get(&ModelId::new("anthropic:claude-haiku-4-5-20251001"))
        .unwrap();
    assert_eq!(dated.input_per_million, dated_price);
    let alias = table
        .get(&ModelId::new("anthropic:claude-haiku-4-5"))
        .unwrap();
    assert_eq!(alias.input_per_million, base);
}
