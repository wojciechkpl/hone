#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier, config side (spec H8): a user `[pricing."anthropic:claude-haiku-5-5"]`
//! entry resolves with no tier, so the user's flat rates apply to every prompt length, and the
//! config schema does not gain a tier key.

use hone_config::resolve::{resolve, ConfigLayer};

const USER: &str = "/home/u/.config/hone/config.toml";

#[test]
fn test_h8_a_user_entry_for_haiku_5_5_should_resolve_without_a_tier() {
    let layer = ConfigLayer::from_toml(
        USER,
        "[pricing.\"anthropic:claude-haiku-5-5\"]\ninput_per_million = 0.3\noutput_per_million = 1.5\n",
    )
    .expect("parses");

    let e = resolve(Some(&layer), None, None, &|_| None).expect("resolves");

    assert_eq!(e.pricing.len(), 1);
    let p = &e.pricing[0];
    assert_eq!(p.model.as_str(), "anthropic:claude-haiku-5-5");
    assert_eq!(
        p.long_prompt, None,
        "the user's flat rates apply to every prompt length"
    );
    assert_eq!(p.calculate_cost(1_000_000, 0, 0, 0), 0.3);
}

#[test]
fn test_h8_a_long_prompt_key_in_a_pricing_table_should_be_rejected_as_unknown() {
    for extra in [
        "long_prompt = { above_prompt_tokens = 100000, input_per_million = 1.0, output_per_million = 2.0 }\n",
        "above_prompt_tokens = 100000\n",
    ] {
        let raw = format!(
            "[pricing.\"anthropic:claude-haiku-5-5\"]\ninput_per_million = 0.3\noutput_per_million = 1.5\n{extra}"
        );
        let r = ConfigLayer::from_toml(USER, &raw);
        assert!(r.is_err(), "unknown key must be rejected: {raw}");
    }
}
