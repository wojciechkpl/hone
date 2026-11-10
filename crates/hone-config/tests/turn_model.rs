#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G7 and G8 (`docs/plans/routing-fallback-spec.md`): the pure functions the CLI and the TUI
//! call to pick a turn's model and to validate `/model`.
//!
//! API (spec section 3), in `hone_config::resolve`:
//! `turn_model(&Effective, Tier, Option<&ModelId>, &ModelId, &dyn Fn(&ModelId) -> bool) -> TurnModel`
//! and `check_model_switch(&str, &dyn Fn(&ModelId) -> bool) -> Result<ModelId, String>`.
//! Own file: it does not compile until they exist.

use std::path::PathBuf;

use hone_config::resolve::{ConfigLayer, Effective, Tier, check_model_switch, resolve, turn_model};
use hone_proto::ModelId;

const USER_PATH: &str = "/home/u/.config/hone/config.toml";
const PROJECT_PATH: &str = "/repo/.hone/config.toml";

fn eff(user_raw: &str, project_raw: Option<&str>) -> Effective {
    let u = ConfigLayer::from_toml(USER_PATH, user_raw).unwrap();
    let p = project_raw.map(|r| ConfigLayer::from_toml(PROJECT_PATH, r).unwrap());
    resolve(Some(&u), p.as_ref(), None, &|_: &str| None).unwrap()
}

const ROUTING: &str = r#"
[[routing]]
task = "simple"
model = "ollama:qwen"
[[routing]]
task = "complex"
model = "anthropic:opus"
"#;

fn id(s: &str) -> ModelId {
    ModelId::new(s)
}

fn all(_: &ModelId) -> bool {
    true
}

fn none(_: &ModelId) -> bool {
    false
}

// -- G7 ---------------------------------------------------------------------------------------------

#[test]
fn test_g7_pinned_model_wins_over_routing_and_default() {
    let e = eff(ROUTING, None);
    let pinned = id("openai:pinned");
    for tier in [Tier::Simple, Tier::Medium, Tier::Complex] {
        let t = turn_model(&e, tier, Some(&pinned), &id("openai:default"), &all);
        assert_eq!(t.model.as_str(), "openai:pinned", "{tier:?}");
        assert_eq!(t.routed_from, None, "{tier:?}");
    }
}

#[test]
fn test_g7_pinned_model_is_returned_even_when_its_provider_is_not_available() {
    let e = eff(ROUTING, None);
    let pinned = id("openai:pinned");
    let t = turn_model(
        &e,
        Tier::Simple,
        Some(&pinned),
        &id("openai:default"),
        &none,
    );
    assert_eq!(t.model.as_str(), "openai:pinned");
}

#[test]
fn test_g7_routed_model_is_used_when_configured_and_available() {
    let e = eff(ROUTING, None);

    let simple = turn_model(&e, Tier::Simple, None, &id("openai:default"), &all);
    assert_eq!(simple.model.as_str(), "ollama:qwen");
    assert_eq!(simple.routed_from, Some(PathBuf::from(USER_PATH)));

    let complex = turn_model(&e, Tier::Complex, None, &id("openai:default"), &all);
    assert_eq!(complex.model.as_str(), "anthropic:opus");
    assert_eq!(complex.routed_from, Some(PathBuf::from(USER_PATH)));
}

#[test]
fn test_g7_a_tier_without_an_entry_uses_the_default() {
    let e = eff(ROUTING, None);
    let t = turn_model(&e, Tier::Medium, None, &id("openai:default"), &all);
    assert_eq!(t.model.as_str(), "openai:default");
    assert_eq!(t.routed_from, None);
}

#[test]
fn test_g7_a_routed_model_whose_provider_is_unavailable_falls_back_to_the_default() {
    let e = eff(ROUTING, None);
    let only_ollama =
        |m: &ModelId| m.as_str().starts_with("ollama:") || m.as_str().starts_with("openai:");
    let t = turn_model(&e, Tier::Complex, None, &id("openai:default"), &only_ollama);
    assert_eq!(t.model.as_str(), "openai:default");
    assert_eq!(t.routed_from, None);

    let t = turn_model(&e, Tier::Simple, None, &id("openai:default"), &only_ollama);
    assert_eq!(
        t.model.as_str(),
        "ollama:qwen",
        "an available routed model still applies"
    );
}

#[test]
fn test_g7_without_any_routing_the_default_is_used_for_every_tier() {
    let e = eff("default_model = \"openai:x\"\n", None);
    for tier in [Tier::Simple, Tier::Medium, Tier::Complex] {
        let t = turn_model(&e, tier, None, &id("openai:default"), &all);
        assert_eq!(t.model.as_str(), "openai:default");
        assert_eq!(t.routed_from, None);
    }
}

#[test]
fn test_g7_the_project_files_entry_reports_its_own_path() {
    let e = eff(
        ROUTING,
        Some("[[routing]]\ntask = \"simple\"\nmodel = \"ollama:proj\"\n"),
    );
    let t = turn_model(&e, Tier::Simple, None, &id("openai:default"), &all);
    assert_eq!(t.model.as_str(), "ollama:proj");
    assert_eq!(t.routed_from, Some(PathBuf::from(PROJECT_PATH)));
    let t = turn_model(&e, Tier::Complex, None, &id("openai:default"), &all);
    assert_eq!(t.routed_from, Some(PathBuf::from(USER_PATH)));
}

// -- G8 -------------------------------------------------------------------------------------------------

#[test]
fn test_g8_an_available_api_model_is_accepted() {
    for m in [
        "anthropic:claude-sonnet-4-5",
        "openai:gpt-4o",
        "ollama:qwen2.5-coder:14b",
    ] {
        let got = check_model_switch(m, &all).unwrap_or_else(|e| panic!("{m}: {e}"));
        assert_eq!(got.as_str(), m);
    }
}

#[test]
fn test_g8_an_agent_cli_prefix_is_refused_with_a_message() {
    for m in ["claude-cli:claude-sonnet-4-5", "gemini-cli:gemini-2.5-pro"] {
        let e = check_model_switch(m, &all).expect_err(m);
        assert!(e.to_lowercase().contains("cli"), "{m}: {e}");
    }
}

#[test]
fn test_g8_a_provider_without_a_key_is_refused_and_named() {
    let only_openai = |m: &ModelId| m.as_str().starts_with("openai:");
    let e = check_model_switch("anthropic:claude-sonnet-4-5", &only_openai)
        .expect_err("no anthropic key");
    assert!(e.to_lowercase().contains("anthropic"), "{e}");
    assert!(check_model_switch("openai:gpt-4o", &only_openai).is_ok());
}

#[test]
fn test_g8_unknown_prefix_and_bad_names_are_refused() {
    for m in [
        "gpt-4o",
        "mistral:x",
        "openai:",
        "openai:-x",
        "ollama:has space",
        "",
    ] {
        let e = check_model_switch(m, &all).expect_err(m);
        assert!(!e.is_empty(), "{m:?}");
    }
}

#[test]
fn test_g8_availability_is_asked_about_the_parsed_model() {
    let seen = std::cell::RefCell::new(Vec::<String>::new());
    let probe = |m: &ModelId| {
        seen.borrow_mut().push(m.as_str().to_owned());
        true
    };
    check_model_switch("openai:gpt-4o", &probe).unwrap();
    assert_eq!(seen.borrow().as_slice(), ["openai:gpt-4o"]);
}
