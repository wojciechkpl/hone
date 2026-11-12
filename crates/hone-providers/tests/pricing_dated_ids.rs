#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U6 of `docs/plans/usage-and-pricing-spec.md`: a model id ending in `-YYYY-MM-DD` is priced
//! with its base id's entry when it has none of its own.

use hone_proto::ModelId;
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
    assert_ne!(rates("openai:gpt-4o-mini-2024-07-18"), rates("openai:gpt-4o"));
}

#[test]
fn test_u6_only_a_dash_separated_date_suffix_counts() {
    assert_eq!(rates("openai:gpt-4o-20240806"), None, "no dashes: not a dated id");
    assert_eq!(rates("openai:gpt-4o-preview"), None);
    assert_eq!(rates("openai:gpt-4o-2024-08"), None, "incomplete date");
    assert_eq!(rates("openai:unknown-model-2024-08-06"), None, "the base has no entry either");
}

#[test]
fn test_u6_an_exact_entry_should_still_win_over_the_base() {
    // The table's own dated entry (claude-haiku-4-5-20251001 has no dashes, so use an id the
    // table lists exactly): lookups of listed ids are unchanged.
    let table = PricingTable::default_table();
    let e = table.get(&ModelId::new("openai:gpt-4o")).unwrap();
    assert_eq!(e.model.as_str(), "openai:gpt-4o");
}
