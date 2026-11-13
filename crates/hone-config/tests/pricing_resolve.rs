#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U2 of `docs/plans/usage-and-pricing-spec.md`, config half: user `[pricing."provider:model"]`
//! tables, resolved by `hone_config::resolve::resolve`.
//!
//! API (new): `Config.pricing` (a map keyed by `provider:model`, parsed from
//! `[pricing."<provider:model>"]` with `input_per_million`, `output_per_million` and optional
//! `cache_read_per_million`, `cache_creation_per_million`) and
//! `Effective.pricing: Vec<hone_proto::PricingEntry>` - the user's entries, sorted by model id,
//! `model` set from the table key. A project file's `[pricing]` parses but is ignored, with a
//! `Warning` naming that file and mentioning `pricing`.

use std::path::Path;

use hone_config::ConfigError;
use hone_config::resolve::{ConfigLayer, Effective, resolve};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}

fn run(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> Effective {
    resolve(u, p, None, &|_| None).unwrap_or_else(|e| panic!("resolve should succeed: {e}"))
}

fn err(u: Option<&ConfigLayer>) -> (String, String) {
    match resolve(u, None, None, &|_| None).expect_err("should fail") {
        ConfigError::InvalidField { path, reason } => (path.display().to_string(), reason),
        other => panic!("{other:?}"),
    }
}

#[test]
fn test_u2_a_user_pricing_table_should_resolve_to_entries() {
    let u = layer(
        USER,
        r#"
[pricing."openai:my-model"]
input_per_million = 1.5
output_per_million = 6.0
cache_read_per_million = 0.15

[pricing."ollama:llama3"]
input_per_million = 0.0
output_per_million = 0.0
"#,
    );

    let e = run(Some(&u), None);

    let ids: Vec<&str> = e.pricing.iter().map(|p| p.model.as_str()).collect();
    assert_eq!(ids, ["ollama:llama3", "openai:my-model"], "sorted by model id");
    let m = &e.pricing[1];
    assert_eq!(
        (m.input_per_million, m.output_per_million, m.cache_read_per_million, m.cache_creation_per_million),
        (1.5, 6.0, Some(0.15), None)
    );
}

#[test]
fn test_u2_no_pricing_table_should_resolve_to_no_entries() {
    assert!(run(None, None).pricing.is_empty());
    assert!(run(Some(&layer(USER, "")), None).pricing.is_empty());
}

#[test]
fn test_u2_a_project_pricing_table_should_be_ignored_with_a_warning_naming_the_file() {
    let p = layer(
        PROJECT,
        "[pricing.\"openai:my-model\"]\ninput_per_million = 0.0\noutput_per_million = 0.0\n",
    );

    let e = run(None, Some(&p));

    assert!(e.pricing.is_empty(), "a repository cannot set prices (it could hide spend from [budget])");
    assert!(
        e.warnings
            .iter()
            .any(|w| w.path == Path::new(PROJECT) && w.message.contains("pricing")),
        "{:?}",
        e.warnings
    );
}

#[test]
fn test_u2_the_users_entries_should_stand_when_a_project_file_has_pricing_too() {
    let u = layer(USER, "[pricing.\"openai:m\"]\ninput_per_million = 2.0\noutput_per_million = 3.0\n");
    let p = layer(PROJECT, "[pricing.\"openai:m\"]\ninput_per_million = 0.0\noutput_per_million = 0.0\n");

    let e = run(Some(&u), Some(&p));

    assert_eq!(e.pricing.len(), 1);
    assert_eq!(e.pricing[0].input_per_million, 2.0);
}

#[test]
fn test_u2_bad_pricing_should_be_an_error_naming_the_file_and_table() {
    for (raw, needle) in [
        ("[pricing.\"openai:m\"]\ninput_per_million = -1.0\noutput_per_million = 1.0\n", "input_per_million"),
        ("[pricing.\"openai:m\"]\ninput_per_million = 1.0\noutput_per_million = -2.0\n", "output_per_million"),
        ("[pricing.\"no-prefix\"]\ninput_per_million = 1.0\noutput_per_million = 1.0\n", "no-prefix"),
    ] {
        let u = layer(USER, raw);

        let (path, reason) = err(Some(&u));

        assert_eq!(path, USER, "{raw}");
        assert!(reason.contains(needle), "{needle} in: {reason}");
        assert!(reason.contains("pricing"), "{reason}");
    }
}

#[test]
fn test_u2_a_pricing_entry_missing_a_rate_should_be_a_parse_error_naming_the_file() {
    let e = ConfigLayer::from_toml(
        USER,
        "[pricing.\"openai:m\"]\ninput_per_million = 1.0\n",
    )
    .expect_err("output_per_million is required");

    match e {
        ConfigError::InvalidField { path, reason } => {
            assert_eq!(path.display().to_string(), USER);
            assert!(reason.contains("output_per_million"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}
