#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The built-in Anthropic price table matches Anthropic's published pricing (fetched 2026-11-17).
//!
//! Hone writes only the 5-minute prompt cache, so `cache_creation_per_million` is the 5m write
//! price. `claude-cli:` rows mirror `anthropic:` rows. Claude Haiku 5.5 is priced at its
//! <=100K-token rate (tier support is a later change).

use hone_proto::ModelId;
use hone_providers::PricingTable;

const EPS: f64 = 1e-9;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

/// (id after the provider prefix, input, output, cache_read, cache_creation_5m)
const ROWS: &[(&str, f64, f64, f64, f64)] = &[
    ("claude-fable-5-2", 8.0, 40.0, 0.20, 10.0),
    ("claude-mythos-5-2", 8.0, 40.0, 0.20, 10.0),
    ("claude-fable-5-1", 10.0, 50.0, 0.25, 12.50),
    ("claude-mythos-5-1", 10.0, 50.0, 0.25, 12.50),
    ("claude-fable-5", 10.0, 50.0, 1.0, 12.50),
    ("claude-mythos-5", 10.0, 50.0, 1.0, 12.50),
    ("claude-opus-5-5", 4.0, 20.0, 0.20, 5.0),
    ("claude-opus-5", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-8", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-7", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-6", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-5", 5.0, 25.0, 0.50, 6.25),
    ("claude-sonnet-5-5", 2.0, 10.0, 0.10, 2.50),
    ("claude-sonnet-5", 2.0, 10.0, 0.20, 2.50),
    ("claude-sonnet-4-6", 3.0, 15.0, 0.30, 3.75),
    ("claude-sonnet-4-5", 3.0, 15.0, 0.30, 3.75),
    ("claude-haiku-5-5", 0.10, 0.50, 0.01, 0.125),
    ("claude-haiku-4-5", 1.0, 5.0, 0.10, 1.25),
];

fn check_row(prefix: &str, row: &(&str, f64, f64, f64, f64)) -> Vec<String> {
    let (id, input, output, read, create) = *row;
    let full = format!("{prefix}:{id}");
    let table = PricingTable::default_table();
    let Some(e) = table.get(&ModelId::new(&full)) else {
        return vec![format!("{full}: missing from the table")];
    };
    let mut errs = Vec::new();
    if !close(e.input_per_million, input) {
        errs.push(format!("{full}: input {} != {input}", e.input_per_million));
    }
    if !close(e.output_per_million, output) {
        errs.push(format!(
            "{full}: output {} != {output}",
            e.output_per_million
        ));
    }
    match e.cache_read_per_million {
        Some(v) if close(v, read) => {}
        other => errs.push(format!("{full}: cache_read {other:?} != Some({read})")),
    }
    match e.cache_creation_per_million {
        Some(v) if close(v, create) => {}
        other => errs.push(format!(
            "{full}: cache_creation {other:?} != Some({create})"
        )),
    }
    errs
}

fn check_all(prefix: &str) {
    let errs: Vec<String> = ROWS.iter().flat_map(|r| check_row(prefix, r)).collect();
    assert!(
        errs.is_empty(),
        "wrong {prefix} prices:\n{}",
        errs.join("\n")
    );
}

#[test]
fn test_anthropic_rows_should_match_published_prices() {
    check_all("anthropic");
}

#[test]
fn test_claude_cli_rows_should_mirror_anthropic_prices() {
    check_all("claude-cli");
}

#[test]
fn test_haiku_4_5_alias_should_be_priced() {
    for prefix in ["anthropic", "claude-cli"] {
        let id = format!("{prefix}:claude-haiku-4-5");
        assert!(
            PricingTable::default_table()
                .get(&ModelId::new(&id))
                .is_some(),
            "{id} has no price"
        );
    }
}

#[test]
fn test_dated_ids_should_resolve_to_their_base_entry() {
    let table = PricingTable::default_table();
    let cases = [
        ("claude-haiku-4-5-20251001", "claude-haiku-4-5"),
        ("claude-opus-4-5-20251101", "claude-opus-4-5"),
        ("claude-sonnet-4-5-20250929", "claude-sonnet-4-5"),
    ];
    for prefix in ["anthropic", "claude-cli"] {
        for (dated, base) in cases {
            let d = table
                .get(&ModelId::new(format!("{prefix}:{dated}")))
                .unwrap_or_else(|| panic!("{prefix}:{dated} has no price"));
            let b = table
                .get(&ModelId::new(format!("{prefix}:{base}")))
                .unwrap_or_else(|| panic!("{prefix}:{base} has no price"));
            assert!(
                close(d.input_per_million, b.input_per_million)
                    && close(d.output_per_million, b.output_per_million)
                    && d.cache_read_per_million == b.cache_read_per_million
                    && d.cache_creation_per_million == b.cache_creation_per_million,
                "{prefix}:{dated} should price like {prefix}:{base}"
            );
        }
    }
}

#[test]
fn test_both_cache_fields_should_be_some_for_every_row() {
    let table = PricingTable::default_table();
    for prefix in ["anthropic", "claude-cli"] {
        for (id, ..) in ROWS {
            let full = format!("{prefix}:{id}");
            let e = table
                .get(&ModelId::new(&full))
                .unwrap_or_else(|| panic!("{full} missing"));
            assert!(
                e.cache_read_per_million.is_some(),
                "{full}: cache_read is None"
            );
            assert!(
                e.cache_creation_per_million.is_some(),
                "{full}: cache_creation is None"
            );
        }
    }
}

#[test]
fn test_opus_4_7_million_in_million_out_should_cost_30_dollars() {
    let table = PricingTable::default_table();
    let e = table
        .get(&ModelId::new("anthropic:claude-opus-4-7"))
        .expect("opus-4-7 entry should exist");
    let cost = e.calculate_cost(1_000_000, 1_000_000, 0, 0);
    assert!(close(cost, 30.0), "expected $30.00, got ${cost}");
}
