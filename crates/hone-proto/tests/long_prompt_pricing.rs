#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier (spec H1-H3): `PricingEntry::long_prompt`, tier selection in
//! `calculate_cost`, and `calculate_cost_with_prompt`.
//!
//! Interface assumed (the implementer must honor it):
//! `hone_proto::LongPromptRates { above_prompt_tokens: u64, input_per_million: f64,
//! output_per_million: f64, cache_read_per_million: Option<f64>,
//! cache_creation_per_million: Option<f64> }` re-exported at the crate root, and
//! `PricingEntry::calculate_cost_with_prompt(prompt_tokens: u64, input: u32, output: u32,
//! cache_read: u32, cache_creation: u32) -> f64`.

use hone_proto::{LongPromptRates, ModelId, PricingEntry};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

/// Relative tolerance, for results in the thousands where one f64 step exceeds 1e-12.
fn close_rel(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

fn tier() -> LongPromptRates {
    LongPromptRates {
        above_prompt_tokens: 100_000,
        input_per_million: 10.0,
        output_per_million: 20.0,
        cache_read_per_million: Some(1.0),
        cache_creation_per_million: Some(12.5),
    }
}

/// Base 1 / 2 / 0.1 / 1.25 per million, with the tier above.
fn tiered() -> PricingEntry {
    PricingEntry {
        model: ModelId::new("test:tiered"),
        input_per_million: 1.0,
        output_per_million: 2.0,
        cache_read_per_million: Some(0.1),
        cache_creation_per_million: Some(1.25),
        long_prompt: Some(tier()),
    }
}

fn flat() -> PricingEntry {
    PricingEntry {
        long_prompt: None,
        ..tiered()
    }
}

fn base_cost(i: u32, o: u32, r: u32, c: u32) -> f64 {
    (i as f64 * 1.0 + o as f64 * 2.0 + r as f64 * 0.1 + c as f64 * 1.25) / 1_000_000.0
}

fn long_cost(i: u32, o: u32, r: u32, c: u32) -> f64 {
    (i as f64 * 10.0 + o as f64 * 20.0 + r as f64 * 1.0 + c as f64 * 12.5) / 1_000_000.0
}

// ---- H1: serde -------------------------------------------------------------------------------

#[test]
fn test_h1_an_entry_without_a_tier_should_serialize_without_a_long_prompt_key() {
    let json = serde_json::to_string(&flat()).unwrap();
    assert!(!json.contains("long_prompt"), "no key when None: {json}");
}

#[test]
fn test_h1_json_without_the_key_should_deserialize_to_none() {
    let old = r#"{"model":"test:old","input_per_million":1.0,"output_per_million":2.0,
                  "cache_read_per_million":0.1,"cache_creation_per_million":1.25}"#;
    let e: PricingEntry = serde_json::from_str(old).unwrap();
    assert_eq!(e.long_prompt, None);
    assert_eq!(e.input_per_million, 1.0);
}

#[test]
fn test_h1_an_entry_with_a_tier_should_round_trip_and_name_its_fields() {
    let e = tiered();
    let json = serde_json::to_value(&e).unwrap();
    let t = &json["long_prompt"];
    assert_eq!(t["above_prompt_tokens"], 100_000);
    assert_eq!(t["input_per_million"], 10.0);
    assert_eq!(t["output_per_million"], 20.0);
    assert_eq!(t["cache_read_per_million"], 1.0);
    assert_eq!(t["cache_creation_per_million"], 12.5);
    let back: PricingEntry = serde_json::from_value(json).unwrap();
    assert_eq!(back, e);
}

// ---- H2: tier selection ----------------------------------------------------------------------

#[test]
fn test_h2_a_prompt_of_exactly_the_threshold_should_use_the_base_rates() {
    // Exactly 100,000 is the lower row, however the prompt is split across categories.
    for (i, r, c) in [
        (100_000, 0, 0),
        (50_000, 50_000, 0),
        (0, 0, 100_000),
        (40_000, 30_000, 30_000),
    ] {
        let got = tiered().calculate_cost(i, 1_000, r, c);
        assert!(
            close(got, base_cost(i, 1_000, r, c)),
            "({i},{r},{c}): {got}"
        );
    }
}

#[test]
fn test_h2_a_prompt_one_token_over_the_threshold_should_use_the_long_rates_for_every_category() {
    for (i, r, c) in [
        (100_001, 0, 0),
        (50_000, 50_001, 0),
        (0, 0, 100_001),
        (40_000, 30_000, 30_001),
    ] {
        let got = tiered().calculate_cost(i, 1_000, r, c);
        assert!(
            close(got, long_cost(i, 1_000, r, c)),
            "({i},{r},{c}): {got}"
        );
    }
}

#[test]
fn test_h2_output_tokens_should_not_count_toward_the_prompt_length() {
    let got = tiered().calculate_cost(100_000, 5_000_000, 0, 0);
    assert!(close(got, base_cost(100_000, 5_000_000, 0, 0)), "{got}");
}

#[test]
fn test_h2_the_prompt_sum_should_not_overflow_u32() {
    // MAX + 10 wraps to 9 in u32: a wrapping sum would pick the base rates.
    let got = tiered().calculate_cost(u32::MAX, 0, 10, 0);
    assert!(close_rel(got, long_cost(u32::MAX, 0, 10, 0)), "{got}");
    // Three near-MAX fields must not panic (debug) either.
    let got = tiered().calculate_cost(u32::MAX, 0, u32::MAX, u32::MAX);
    assert!(
        close_rel(got, long_cost(u32::MAX, 0, u32::MAX, u32::MAX)),
        "{got}"
    );
}

#[test]
fn test_h2_a_missing_cache_read_rate_in_the_tier_should_cost_nothing() {
    let mut e = tiered();
    e.long_prompt = Some(LongPromptRates {
        cache_read_per_million: None,
        ..tier()
    });
    let got = e.calculate_cost(60_000, 0, 50_000, 0);
    assert!(close(got, 60_000.0 * 10.0 / 1e6), "{got}");
}

#[test]
fn test_h2_a_missing_cache_write_rate_in_the_tier_should_be_1_25_times_the_tier_input_rate() {
    let mut e = tiered();
    e.long_prompt = Some(LongPromptRates {
        cache_creation_per_million: None,
        ..tier()
    });
    let got = e.calculate_cost(60_000, 0, 0, 50_000);
    // tier input 10.0 -> write 12.5 (not 1.25 x the base input rate)
    assert!(
        close(got, (60_000.0 * 10.0 + 50_000.0 * 12.5) / 1e6),
        "{got}"
    );
}

#[test]
fn test_h2_a_missing_cache_write_rate_in_the_base_should_still_derive_from_the_base_input_rate() {
    let mut e = tiered();
    e.cache_creation_per_million = None;
    let got = e.calculate_cost(10_000, 0, 0, 20_000);
    assert!(
        close(got, (10_000.0 * 1.0 + 20_000.0 * 1.25) / 1e6),
        "{got}"
    );
}

#[test]
fn test_h2_without_a_tier_the_result_should_be_the_old_formula_bit_for_bit() {
    let e = flat();
    for (i, o, r, c) in [
        (1_000_000, 1_000_000, 0, 0),
        (500_000, 7, 300_000, 400_000),
        (u32::MAX, u32::MAX, 5, 9),
    ] {
        let old = (i as f64 / 1_000_000.0) * 1.0
            + (o as f64 / 1_000_000.0) * 2.0
            + (r as f64 / 1_000_000.0) * 0.1
            + (c as f64 / 1_000_000.0) * 1.25;
        assert_eq!(e.calculate_cost(i, o, r, c), old, "({i},{o},{r},{c})");
    }
}

#[test]
fn test_h2_the_threshold_should_come_from_the_tier() {
    let mut e = tiered();
    e.long_prompt = Some(LongPromptRates {
        above_prompt_tokens: 10,
        ..tier()
    });
    assert!(close(e.calculate_cost(10, 0, 0, 0), base_cost(10, 0, 0, 0)));
    assert!(close(e.calculate_cost(11, 0, 0, 0), long_cost(11, 0, 0, 0)));
}

// ---- H3: calculate_cost_with_prompt ----------------------------------------------------------

#[test]
fn test_h3_an_output_only_event_of_a_long_request_should_use_the_long_output_rate() {
    let got = tiered().calculate_cost_with_prompt(110_000, 0, 1_000, 0, 0);
    assert!(close(got, long_cost(0, 1_000, 0, 0)), "{got}");
}

#[test]
fn test_h3_a_known_prompt_of_exactly_the_threshold_should_use_the_base_rates() {
    let got = tiered().calculate_cost_with_prompt(100_000, 0, 1_000, 0, 0);
    assert!(close(got, base_cost(0, 1_000, 0, 0)), "{got}");
}

#[test]
fn test_h3_the_tier_should_follow_prompt_tokens_not_the_arguments() {
    // Big arguments, small known prompt: base. Small arguments, big known prompt: long.
    let small = tiered().calculate_cost_with_prompt(1, 500_000, 0, 0, 0);
    assert!(close(small, base_cost(500_000, 0, 0, 0)), "{small}");
    let big = tiered().calculate_cost_with_prompt(u64::MAX, 5, 0, 0, 0);
    assert!(close(big, long_cost(5, 0, 0, 0)), "{big}");
}

#[test]
fn test_h3_without_a_tier_every_prompt_length_should_use_the_base_rates() {
    let got = flat().calculate_cost_with_prompt(5_000_000, 100, 200, 300, 400);
    assert!(close(got, base_cost(100, 200, 300, 400)), "{got}");
}

#[test]
fn test_h3_calculate_cost_should_equal_the_prompt_variant_with_the_arguments_prompt() {
    for (i, o, r, c) in [
        (60_000, 10, 0, 50_000),
        (90_000, 1_000, 10_000, 0),
        (1, 2, 3, 4),
    ] {
        let e = tiered();
        assert_eq!(
            e.calculate_cost(i, o, r, c),
            e.calculate_cost_with_prompt(i as u64 + r as u64 + c as u64, i, o, r, c)
        );
    }
}
