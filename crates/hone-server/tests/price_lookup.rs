#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U15 of `docs/plans/usage-and-pricing-spec.md`: `honed` sets the process default price lookup
//! from the default table plus the user's `[pricing]`.
//!
//! `honed`'s `main` is not testable, so the construction is a function:
//! `hone_server::build_price_lookup(user: Option<&ConfigLayer>, project: Option<&ConfigLayer>)
//! -> Result<hone_core::PriceLookup, hone_config::ConfigError>`. It resolves the layers like the
//! CLI does (`hone_config::resolve::resolve`: user entries only, a project's `[pricing]` ignored),
//! overlays them on `PricingTable::default_table()` and returns the lookup `honed` installs with
//! `set_default_price_lookup`.

use hone_config::resolve::ConfigLayer;
use hone_proto::ModelId;
use hone_server::build_price_lookup;

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap()
}

fn rates(lookup: &hone_core::PriceLookup, model: &str) -> Option<(f64, f64)> {
    lookup(&ModelId::new(model)).map(|e| (e.input_per_million, e.output_per_million))
}

#[test]
fn test_u15_the_users_pricing_table_should_price_its_models() {
    let user = layer(USER, "[pricing.\"openai:my-model\"]\ninput_per_million = 1.5\noutput_per_million = 6.0\n");

    let lookup = build_price_lookup(Some(&user), None).unwrap();

    assert_eq!(rates(&lookup, "openai:my-model"), Some((1.5, 6.0)));
    assert_eq!(rates(&lookup, "openai:gpt-4o"), Some((2.5, 10.0)), "the default table is still there");
    assert_eq!(rates(&lookup, "ollama:llama3"), None, "unknown models stay unpriced");
}

#[test]
fn test_u15_a_user_entry_should_override_the_default_table() {
    let user = layer(USER, "[pricing.\"openai:gpt-4o\"]\ninput_per_million = 1.0\noutput_per_million = 2.0\n");

    let lookup = build_price_lookup(Some(&user), None).unwrap();

    assert_eq!(rates(&lookup, "openai:gpt-4o"), Some((1.0, 2.0)));
}

#[test]
fn test_u15_a_projects_pricing_table_should_be_ignored() {
    let project = layer(PROJECT, "[pricing.\"openai:my-model\"]\ninput_per_million = 0.0\noutput_per_million = 0.0\n");

    let lookup = build_price_lookup(None, Some(&project)).unwrap();

    assert_eq!(rates(&lookup, "openai:my-model"), None);
}

#[test]
fn test_u15_dated_ids_should_resolve_through_the_user_entries_too() {
    let user = layer(USER, "[pricing.\"openai:my-model\"]\ninput_per_million = 1.5\noutput_per_million = 6.0\n");

    let lookup = build_price_lookup(Some(&user), None).unwrap();

    assert_eq!(rates(&lookup, "openai:my-model-2025-01-31"), Some((1.5, 6.0)));
}
