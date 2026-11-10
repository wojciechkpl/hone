#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G1 and G2 (`docs/plans/routing-fallback-spec.md`): `[[routing]]` per complexity tier and
//! `fallback_models`, as exposed by the resolver.
//!
//! API (spec section 3): `hone_config::resolve::{Tier, RoutedModel, TierRouting}`, the new
//! `Effective` fields `routing`, `fallback_models`, `fallback_path`, and
//! `Config.fallback_models: Vec<ModelId>`.
//!
//! Own file: it does not compile until the types exist; the other test files are unaffected.

use std::path::{Path, PathBuf};

use hone_config::resolve::{ConfigLayer, Effective, Tier, Warning, resolve};
use hone_config::{Config, ConfigError};

const USER_PATH: &str = "/home/u/.config/hone/config.toml";
const PROJECT_PATH: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}
fn user(raw: &str) -> ConfigLayer {
    layer(USER_PATH, raw)
}
fn project(raw: &str) -> ConfigLayer {
    layer(PROJECT_PATH, raw)
}
fn no_env(_: &str) -> Option<String> {
    None
}

fn run(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> Effective {
    resolve(u, p, None, &no_env).unwrap_or_else(|e| panic!("resolve should succeed: {e}"))
}

fn run_err(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>) -> ConfigError {
    resolve(u, p, None, &no_env).expect_err("resolve should fail")
}

fn err_path(e: &ConfigError) -> &Path {
    match e {
        ConfigError::InvalidField { path, .. } => path,
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

fn err_reason(e: &ConfigError) -> &str {
    match e {
        ConfigError::InvalidField { reason, .. } => reason,
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

fn warnings_for<'a>(e: &'a Effective, path: &str) -> Vec<&'a Warning> {
    e.warnings
        .iter()
        .filter(|w| w.path == Path::new(path))
        .collect()
}

fn entry(task: &str, model: &str) -> String {
    format!("[[routing]]\ntask = {task:?}\nmodel = {model:?}\n")
}

fn routed(e: &Effective, tier: Tier) -> Option<(String, PathBuf)> {
    e.routing
        .get(tier)
        .map(|r| (r.model.as_str().to_owned(), r.path.clone()))
}

fn models(e: &Effective) -> Vec<String> {
    e.fallback_models
        .iter()
        .map(|m| m.as_str().to_owned())
        .collect()
}

// -- G1: routing entries ---------------------------------------------------------------------------

#[test]
fn test_g1_no_routing_means_an_empty_table() {
    let e = run(None, None);
    assert!(e.routing.is_empty());
    for tier in [Tier::Simple, Tier::Medium, Tier::Complex] {
        assert!(e.routing.get(tier).is_none());
    }
    let u = user("default_model = \"openai:x\"\n");
    assert!(run(Some(&u), None).routing.is_empty());
}

#[test]
fn test_g1_each_tier_is_exposed_with_the_path_it_came_from() {
    let raw = [
        entry("simple", "ollama:qwen2.5-coder"),
        entry("medium", "anthropic:claude-sonnet-4-5"),
        entry("complex", "openai:gpt-4o"),
    ]
    .join("\n");
    let u = user(&raw);

    let e = run(Some(&u), None);

    assert!(!e.routing.is_empty());
    assert_eq!(
        routed(&e, Tier::Simple),
        Some(("ollama:qwen2.5-coder".to_owned(), PathBuf::from(USER_PATH)))
    );
    assert_eq!(
        routed(&e, Tier::Medium),
        Some((
            "anthropic:claude-sonnet-4-5".to_owned(),
            PathBuf::from(USER_PATH)
        ))
    );
    assert_eq!(
        routed(&e, Tier::Complex),
        Some(("openai:gpt-4o".to_owned(), PathBuf::from(USER_PATH)))
    );
    assert!(
        e.warnings.is_empty(),
        "the old 'not applied' warning is gone: {:?}",
        e.warnings
    );
}

#[test]
fn test_g1_a_single_tier_leaves_the_others_unrouted() {
    let u = user(&entry("complex", "openai:gpt-4o"));
    let e = run(Some(&u), None);
    assert!(e.routing.get(Tier::Simple).is_none());
    assert!(e.routing.get(Tier::Medium).is_none());
    assert!(e.routing.get(Tier::Complex).is_some());
}

#[test]
fn test_g1_project_entry_overrides_the_users_per_task_and_the_users_other_tiers_stay() {
    let u = user(&format!(
        "{}\n{}",
        entry("simple", "ollama:user-simple"),
        entry("complex", "openai:user-complex")
    ));
    let p = project(&entry("simple", "ollama:project-simple"));

    let e = run(Some(&u), Some(&p));

    assert_eq!(
        routed(&e, Tier::Simple),
        Some((
            "ollama:project-simple".to_owned(),
            PathBuf::from(PROJECT_PATH)
        ))
    );
    assert_eq!(
        routed(&e, Tier::Complex),
        Some(("openai:user-complex".to_owned(), PathBuf::from(USER_PATH)))
    );
    assert!(e.warnings.is_empty(), "{:?}", e.warnings);
}

#[test]
fn test_g1_unknown_task_warns_once_and_the_entry_is_ignored() {
    let u = user(&format!(
        "{}\n{}",
        entry("file_read", "anthropic:claude-haiku-3-5"),
        entry("simple", "ollama:x")
    ));

    let e = run(Some(&u), None);

    let ws = warnings_for(&e, USER_PATH);
    assert_eq!(ws.len(), 1, "{:?}", e.warnings);
    assert!(ws[0].message.contains("file_read"), "{}", ws[0].message);
    assert!(
        routed(&e, Tier::Simple).is_some(),
        "the valid entry still applies"
    );
    assert!(e.routing.get(Tier::Medium).is_none() && e.routing.get(Tier::Complex).is_none());
}

#[test]
fn test_g1_task_names_are_exact_and_lowercase() {
    for task in ["Simple", "SIMPLE", "simple ", "easy", ""] {
        let u = user(&entry(task, "ollama:x"));
        let e = run(Some(&u), None);
        assert!(e.routing.is_empty(), "{task:?}");
        assert_eq!(
            warnings_for(&e, USER_PATH).len(),
            1,
            "{task:?}: {:?}",
            e.warnings
        );
    }
}

#[test]
fn test_g1_agent_cli_prefix_warns_and_is_ignored_in_either_file() {
    for model in ["claude-cli:claude-sonnet-4-5", "gemini-cli:gemini-2.5-pro"] {
        let u = user(&entry("simple", model));
        let e = run(Some(&u), None);
        assert!(e.routing.is_empty(), "{model}");
        let ws = warnings_for(&e, USER_PATH);
        assert_eq!(ws.len(), 1, "{model}: {:?}", e.warnings);
        assert!(ws[0].message.contains("cli"), "{model}: {}", ws[0].message);

        let p = project(&entry("simple", model));
        let e = run(None, Some(&p));
        assert!(e.routing.is_empty(), "{model} (project)");
        assert_eq!(
            warnings_for(&e, PROJECT_PATH).len(),
            1,
            "{model}: {:?}",
            e.warnings
        );
    }
}

#[test]
fn test_g1_a_cli_entry_does_not_hide_the_other_files_entry_for_that_tier() {
    let u = user(&entry("simple", "ollama:user-simple"));
    let p = project(&entry("simple", "claude-cli:x"));
    let e = run(Some(&u), Some(&p));
    assert_eq!(
        routed(&e, Tier::Simple),
        Some(("ollama:user-simple".to_owned(), PathBuf::from(USER_PATH)))
    );
}

#[test]
fn test_g1_bad_prefix_or_bad_name_is_a_config_error_naming_the_file() {
    for model in [
        "gpt-4o",
        "mistral:x",
        "",
        "openai:",
        "openai:-x",
        "ollama:has space",
        "anthropic:tab\\there",
    ] {
        let raw = format!("[[routing]]\ntask = \"simple\"\nmodel = \"{model}\"\n");
        let u = ConfigLayer::from_toml(USER_PATH, &raw).unwrap();
        let e = run_err(Some(&u), None);
        assert_eq!(err_path(&e), Path::new(USER_PATH), "{model:?}");
        assert!(err_reason(&e).contains("routing"), "{model:?}: {e}");

        let p = ConfigLayer::from_toml(PROJECT_PATH, &raw).unwrap();
        let e = run_err(None, Some(&p));
        assert_eq!(err_path(&e), Path::new(PROJECT_PATH), "{model:?}");
    }
}

#[test]
fn test_g1_duplicate_task_in_one_file_warns_and_the_first_wins() {
    let u = user(&format!(
        "{}\n{}",
        entry("simple", "ollama:first"),
        entry("simple", "ollama:second")
    ));

    let e = run(Some(&u), None);

    assert_eq!(routed(&e, Tier::Simple).unwrap().0, "ollama:first");
    let ws = warnings_for(&e, USER_PATH);
    assert_eq!(ws.len(), 1, "{:?}", e.warnings);
    assert!(ws[0].message.contains("simple"), "{}", ws[0].message);
}

#[test]
fn test_g1_the_old_fixture_loads_with_two_warnings_and_no_routing() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/full.toml");
    let raw = std::fs::read_to_string(&path).unwrap();
    let l = ConfigLayer::from_toml(&path, &raw).expect("the old fixture still parses");

    let e = run(Some(&l), None);

    assert!(
        e.routing.is_empty(),
        "file_read / multi_file_edit are not tiers"
    );
    let about_tasks = e
        .warnings
        .iter()
        .filter(|w| w.message.contains("file_read") || w.message.contains("multi_file_edit"))
        .count();
    assert_eq!(
        about_tasks, 2,
        "one warning per unknown task: {:?}",
        e.warnings
    );
}

#[test]
fn test_g1_the_config_struct_still_parses_free_form_task_names() {
    let cfg = Config::from_toml_str("[[routing]]\ntask = \"file_read\"\nmodel = \"anthropic:x\"\n")
        .unwrap();
    assert_eq!(cfg.routing.len(), 1);
}

// -- G2: fallback_models -------------------------------------------------------------------------------------

#[test]
fn test_g2_no_list_means_empty_and_no_path() {
    let e = run(None, None);
    assert!(e.fallback_models.is_empty());
    assert_eq!(e.fallback_path, None);
}

#[test]
fn test_g2_a_list_is_exposed_in_order_with_its_source_path() {
    let u = user("fallback_models = [\"anthropic:a\", \"openai:b\", \"ollama:c\"]\n");

    let e = run(Some(&u), None);

    assert_eq!(models(&e), vec!["anthropic:a", "openai:b", "ollama:c"]);
    assert_eq!(e.fallback_path, Some(PathBuf::from(USER_PATH)));
    assert!(e.warnings.is_empty(), "{:?}", e.warnings);
}

#[test]
fn test_g2_the_config_struct_reads_the_key() {
    let cfg = Config::from_toml_str("fallback_models = [\"openai:a\"]\n").unwrap();
    assert_eq!(cfg.fallback_models.len(), 1);
    assert_eq!(cfg.fallback_models[0].as_str(), "openai:a");
    assert!(Config::default().fallback_models.is_empty());
}

#[test]
fn test_g2_bad_prefix_or_name_is_a_config_error() {
    for bad in ["gpt-4o", "mistral:x", "openai:", "openai:-x", "ollama:a b"] {
        let raw = format!("fallback_models = [\"openai:ok\", \"{bad}\"]\n");
        let u = ConfigLayer::from_toml(USER_PATH, &raw).unwrap();
        let e = run_err(Some(&u), None);
        assert_eq!(err_path(&e), Path::new(USER_PATH), "{bad}");
        assert!(err_reason(&e).contains("fallback_models"), "{bad}: {e}");
    }
}

#[test]
fn test_g2_agent_cli_models_warn_and_are_dropped() {
    let u = user("fallback_models = [\"claude-cli:x\", \"openai:b\", \"gemini-cli:y\"]\n");

    let e = run(Some(&u), None);

    assert_eq!(models(&e), vec!["openai:b"]);
    assert_eq!(
        warnings_for(&e, USER_PATH).len(),
        2,
        "one warning per dropped entry: {:?}",
        e.warnings
    );
}

#[test]
fn test_g2_duplicates_are_dropped_keeping_the_first() {
    let u = user("fallback_models = [\"openai:b\", \"ollama:c\", \"openai:b\", \"ollama:c\"]\n");
    assert_eq!(models(&run(Some(&u), None)), vec!["openai:b", "ollama:c"]);
}

#[test]
fn test_g2_a_non_empty_project_list_replaces_the_users() {
    let u = user("fallback_models = [\"openai:user\"]\n");
    let p = project("fallback_models = [\"ollama:project\"]\n");

    let e = run(Some(&u), Some(&p));

    assert_eq!(models(&e), vec!["ollama:project"]);
    assert_eq!(e.fallback_path, Some(PathBuf::from(PROJECT_PATH)));
}

#[test]
fn test_g2_an_empty_or_fully_filtered_project_list_leaves_the_users() {
    let u = user("fallback_models = [\"openai:user\"]\n");
    for raw in [
        "fallback_models = []\n",
        "fallback_models = [\"claude-cli:x\"]\n",
        "default_model = \"openai:m\"\n",
    ] {
        let p = project(raw);
        let e = run(Some(&u), Some(&p));
        assert_eq!(models(&e), vec!["openai:user"], "{raw}");
        assert_eq!(e.fallback_path, Some(PathBuf::from(USER_PATH)), "{raw}");
    }
}

#[test]
fn test_g2_other_unknown_keys_are_still_rejected() {
    for raw in [
        "fallback_model = [\"openai:a\"]\n",
        "fallbacks = []\n",
        "totally_unknown = 1\n",
    ] {
        assert!(ConfigLayer::from_toml(USER_PATH, raw).is_err(), "{raw}");
    }
}
