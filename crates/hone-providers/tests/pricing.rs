#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hone_proto::{ModelId, PricingEntry};
use hone_providers::PricingTable;

#[test]
fn test_default_table_has_known_models() {
    let table = PricingTable::default_table();
    let entry = table
        .get(&ModelId::new("anthropic:claude-opus-4-5"))
        .expect("opus entry should exist");
    assert_eq!(entry.input_per_million, 15.0);
    assert_eq!(entry.output_per_million, 75.0);
}

#[test]
fn test_get_unknown_model_returns_none() {
    let table = PricingTable::default_table();
    assert!(table.get(&ModelId::new("unknown:model")).is_none());
}

#[test]
fn test_insert_overrides_existing() {
    let mut table = PricingTable::default_table();
    table.insert(PricingEntry {
        model: ModelId::new("anthropic:claude-opus-4-5"),
        input_per_million: 1.0,
        output_per_million: 2.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    });
    let entry = table
        .get(&ModelId::new("anthropic:claude-opus-4-5"))
        .expect("overridden entry should exist");
    assert_eq!(entry.input_per_million, 1.0);
    assert_eq!(entry.output_per_million, 2.0);
}

#[test]
fn test_pricing_matches_provider_docs() {
    let table = PricingTable::default_table();
    let entry = table
        .get(&ModelId::new("anthropic:claude-sonnet-4-5"))
        .expect("sonnet entry should exist");
    assert_eq!(entry.input_per_million, 3.0);
    assert_eq!(entry.output_per_million, 15.0);
}
