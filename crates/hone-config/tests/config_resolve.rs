#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for the pure `config.toml` resolver (`docs/plans/config-apply-spec.md`,
//! rows C1-C8, C12, C14/C15 at the library level, and the precedence chains / trust model).
//!
//! PINNED API (module `hone_config::resolve`; the implementer must provide exactly this):
//!
//! ```ignore
//! pub struct ConfigLayer { pub path: PathBuf, pub config: Config }
//! impl ConfigLayer {
//!     /// Parse `raw` as the file at `path`. Errors are `ConfigError::InvalidField { path, reason }`.
//!     pub fn from_toml(path: impl Into<PathBuf>, raw: &str) -> Result<ConfigLayer, ConfigError>;
//!     /// `Ok(None)` if the file does not exist; unreadable / unparsable -> `InvalidField { path, .. }`.
//!     pub fn load(path: &Path) -> Result<Option<ConfigLayer>, ConfigError>;
//! }
//!
//! pub fn resolve(
//!     user: Option<&ConfigLayer>,      // trusted ~/.config/hone/config.toml
//!     project: Option<&ConfigLayer>,   // untrusted <cwd>/.hone/config.toml
//!     cli_model: Option<&str>,         // --model, already known to be non-empty
//!     env: &dyn Fn(&str) -> Option<String>,
//! ) -> Result<Effective, ConfigError>; // every C12 error is `InvalidField { path, reason }`
//!
//! pub struct Effective {
//!     pub model: Option<ModelChoice>,            // None => auto-detect
//!     pub anthropic: ProviderSettings,
//!     pub openai: ProviderSettings,              // also serves deepseek / openrouter
//!     pub ollama: ProviderSettings,
//!     pub session_limit_usd: Option<f64>,
//!     pub warnings: Vec<Warning>,
//! }
//! pub struct ModelChoice { pub model: ModelId, pub source: ModelSource }
//! pub enum ModelSource { Cli, DefaultModel { path: PathBuf }, Wizard { path: PathBuf } }
//! pub struct ProviderSettings {
//!     pub api_key: Option<String>,       // resolved key (None = none available)
//!     pub api_key_env: Option<String>,   // variable consulted (user override or default); None for ollama
//!     pub base_url: Option<String>,      // ollama: Some("http://localhost:11434") by default
//!     pub default_model: String,         // model when auto-detection picks this provider
//! }
//! pub struct Warning { pub path: PathBuf, pub message: String }
//!
//! // in `Config` (types.rs): the wizard table, accepted by the strict schema (C6)
//! pub provider: Option<WizardProviderConfig>,
//! pub struct WizardProviderConfig {
//!     pub name: Option<String>, pub api_key: Option<String>,
//!     pub model: Option<String>, pub base_url: Option<String>,
//! }  // deny_unknown_fields
//! ```
//!
//! Key-shaped fixtures are built at runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hone_config::resolve::{
    ConfigLayer, Effective, ModelChoice, ModelSource, ProviderSettings, Warning, resolve,
};
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

fn env_of(pairs: &[(&str, String)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    move |k: &str| map.get(k).cloned()
}

fn no_env() -> impl Fn(&str) -> Option<String> {
    |_: &str| None
}

fn run(u: Option<&ConfigLayer>, p: Option<&ConfigLayer>, cli: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Effective {
    resolve(u, p, cli, env).unwrap_or_else(|e| panic!("resolve should succeed: {e}"))
}

fn err_path(e: &ConfigError) -> &Path {
    match e {
        ConfigError::InvalidField { path, .. } => path,
        other => panic!("expected ConfigError::InvalidField, got {other:?}"),
    }
}

fn err_reason(e: &ConfigError) -> &str {
    match e {
        ConfigError::InvalidField { reason, .. } => reason,
        other => panic!("expected ConfigError::InvalidField, got {other:?}"),
    }
}

fn model_of(e: &Effective) -> Option<(&str, &ModelSource)> {
    e.model.as_ref().map(|ModelChoice { model, source }| (model.as_str(), source))
}

fn warnings_for<'a>(e: &'a Effective, path: &str) -> Vec<&'a Warning> {
    e.warnings.iter().filter(|w| w.path == Path::new(path)).collect()
}

fn key(prefix: &str) -> String {
    format!("{prefix}-{}", "fixture0123456789")
}

// -- no config at all ----------------------------------------------------------------------------

#[test]
fn test_resolve_with_no_layers_should_use_defaults() {
    let e = run(None, None, None, &no_env());
    assert!(e.model.is_none());
    assert!(e.warnings.is_empty());
    assert_eq!(e.session_limit_usd, None);
    assert_eq!(e.anthropic.default_model, "claude-sonnet-4-5");
    assert_eq!(e.openai.default_model, "gpt-4o");
    assert_eq!(e.ollama.default_model, "llama3.2");
    assert_eq!(e.anthropic.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    assert_eq!(e.openai.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(e.ollama.api_key_env, None);
    assert_eq!(e.anthropic.base_url, None);
    assert_eq!(e.openai.base_url, None);
    assert_eq!(e.ollama.base_url.as_deref(), Some("http://localhost:11434"));
    assert_eq!(e.anthropic.api_key, None);
    assert_eq!(e.openai.api_key, None);
}

// -- C1: default_model -----------------------------------------------------------------------------

#[test]
fn test_c1_default_model_from_user_file_should_be_the_model_choice_with_its_path() {
    let u = user(r#"default_model = "openai:gpt-4o-mini""#);
    let e = run(Some(&u), None, None, &no_env());
    let (m, s) = model_of(&e).expect("a model choice");
    assert_eq!(m, "openai:gpt-4o-mini");
    assert_eq!(*s, ModelSource::DefaultModel { path: PathBuf::from(USER_PATH) });
}

#[test]
fn test_c1_project_default_model_should_win_over_user() {
    let u = user(r#"default_model = "openai:user-model""#);
    let p = project(r#"default_model = "anthropic:project-model""#);
    let e = run(Some(&u), Some(&p), None, &no_env());
    let (m, s) = model_of(&e).unwrap();
    assert_eq!(m, "anthropic:project-model");
    assert_eq!(*s, ModelSource::DefaultModel { path: PathBuf::from(PROJECT_PATH) });
    assert!(e.warnings.is_empty(), "a project default_model is allowed: {:?}", e.warnings);
}

#[test]
fn test_c1_cli_model_should_win_over_default_model() {
    let u = user(r#"default_model = "openai:from-file""#);
    let e = run(Some(&u), None, Some("ollama:qwen2.5-coder:14b"), &no_env());
    let (m, s) = model_of(&e).unwrap();
    assert_eq!(m, "ollama:qwen2.5-coder:14b");
    assert_eq!(*s, ModelSource::Cli);
}

#[test]
fn test_c1_all_five_prefixes_are_valid_default_models() {
    for m in [
        "anthropic:claude-sonnet-4-5",
        "openai:gpt-4o",
        "ollama:llama3.2",
        "claude-cli:claude-sonnet-4-5",
        "gemini-cli:gemini-2.5-pro",
    ] {
        let u = user(&format!("default_model = {m:?}"));
        let e = run(Some(&u), None, None, &no_env());
        assert_eq!(model_of(&e).unwrap().0, m);
    }
}

// -- C2: [providers.<p>].model ---------------------------------------------------------------------

#[test]
fn test_c2_provider_model_should_be_the_model_when_detection_picks_that_provider() {
    let u = user(
        r#"
[providers.anthropic]
model = "claude-opus-4-5"
[providers.openai]
model = "gpt-4o-mini"
[providers.ollama]
model = "qwen2.5-coder"
"#,
    );
    let e = run(Some(&u), None, None, &no_env());
    assert_eq!(e.anthropic.default_model, "claude-opus-4-5");
    assert_eq!(e.openai.default_model, "gpt-4o-mini");
    assert_eq!(e.ollama.default_model, "qwen2.5-coder");
    assert!(e.model.is_none(), "[providers.*].model does not make an explicit choice");
}

#[test]
fn test_c2_project_provider_model_should_win_over_user_and_be_allowed() {
    let u = user("[providers.openai]\nmodel = \"user-m\"\n");
    let p = project("[providers.openai]\nmodel = \"project-m\"\n");
    let e = run(Some(&u), Some(&p), None, &no_env());
    assert_eq!(e.openai.default_model, "project-m");
    assert!(e.warnings.is_empty(), "{:?}", e.warnings);
}

#[test]
fn test_c2_openai_model_chain_env_then_providers_then_wizard_then_default() {
    let w = user("[provider]\nname = \"openai\"\nmodel = \"wizard-m\"\n");
    assert_eq!(run(Some(&w), None, None, &no_env()).openai.default_model, "wizard-m");

    let u = user("[providers.openai]\nmodel = \"providers-m\"\n[provider]\nname = \"openai\"\nmodel = \"wizard-m\"\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).openai.default_model, "providers-m");

    let env = env_of(&[("HONE_OPENAI_MODEL", "env-m".to_owned())]);
    assert_eq!(run(Some(&u), None, None, &env).openai.default_model, "env-m");
}

#[test]
fn test_c2_anthropic_model_chain_providers_then_wizard_then_default() {
    let w = user("[provider]\nname = \"anthropic\"\nmodel = \"wizard-claude\"\n");
    assert_eq!(run(Some(&w), None, None, &no_env()).anthropic.default_model, "wizard-claude");
    let u = user("[providers.anthropic]\nmodel = \"providers-claude\"\n[provider]\nname = \"anthropic\"\nmodel = \"wizard-claude\"\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).anthropic.default_model, "providers-claude");
}

#[test]
fn test_c2_wizard_model_applies_only_to_the_provider_the_wizard_names() {
    let w = user("[provider]\nname = \"ollama\"\nmodel = \"mistral\"\n");
    let e = run(Some(&w), None, None, &no_env());
    assert_eq!(e.ollama.default_model, "mistral");
    assert_eq!(e.openai.default_model, "gpt-4o");
    assert_eq!(e.anthropic.default_model, "claude-sonnet-4-5");
}

// -- C3: base_url ----------------------------------------------------------------------------------

#[test]
fn test_c3_user_provider_base_urls_should_apply() {
    let u = user(
        r#"
[providers.anthropic]
base_url = "https://anthropic.example"
[providers.openai]
base_url = "https://openai.example"
[providers.ollama]
base_url = "http://ollama.example:11434"
"#,
    );
    let e = run(Some(&u), None, None, &no_env());
    assert_eq!(e.anthropic.base_url.as_deref(), Some("https://anthropic.example"));
    assert_eq!(e.openai.base_url.as_deref(), Some("https://openai.example"));
    assert_eq!(e.ollama.base_url.as_deref(), Some("http://ollama.example:11434"));
}

#[test]
fn test_c3_project_base_url_should_be_ignored_with_a_warning_naming_the_project_file() {
    let u = user("[providers.openai]\nbase_url = \"https://trusted.example\"\n");
    let p = project("[providers.openai]\nbase_url = \"https://evil.example\"\n[providers.anthropic]\nbase_url = \"https://evil2.example\"\n[providers.ollama]\nbase_url = \"http://evil3\"\n");
    let e = run(Some(&u), Some(&p), None, &no_env());
    assert_eq!(e.openai.base_url.as_deref(), Some("https://trusted.example"));
    assert_eq!(e.anthropic.base_url, None);
    assert_eq!(e.ollama.base_url.as_deref(), Some("http://localhost:11434"));
    let ws = warnings_for(&e, PROJECT_PATH);
    assert_eq!(ws.len(), 3, "one warning per ignored base_url: {:?}", e.warnings);
    for w in ws {
        assert!(w.message.contains("base_url"), "{}", w.message);
    }
}

#[test]
fn test_c3_openai_base_url_chain_env_then_providers_then_wizard_then_none() {
    let w = user("[provider]\nname = \"deepseek\"\nbase_url = \"https://wizard.example\"\n");
    assert_eq!(run(Some(&w), None, None, &no_env()).openai.base_url.as_deref(), Some("https://wizard.example"));

    let u = user("[providers.openai]\nbase_url = \"https://providers.example\"\n[provider]\nname = \"openai\"\nbase_url = \"https://wizard.example\"\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).openai.base_url.as_deref(), Some("https://providers.example"));

    let env = env_of(&[("HONE_OPENAI_BASE_URL", "https://env.example".to_owned())]);
    assert_eq!(run(Some(&u), None, None, &env).openai.base_url.as_deref(), Some("https://env.example"));
}

#[test]
fn test_c3_ollama_base_url_chain_providers_then_wizard_then_default() {
    let w = user("[provider]\nname = \"ollama\"\nbase_url = \"http://wizard-ollama:1\"\n");
    assert_eq!(run(Some(&w), None, None, &no_env()).ollama.base_url.as_deref(), Some("http://wizard-ollama:1"));
    let u = user("[providers.ollama]\nbase_url = \"http://providers-ollama:2\"\n[provider]\nname = \"ollama\"\nbase_url = \"http://wizard-ollama:1\"\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).ollama.base_url.as_deref(), Some("http://providers-ollama:2"));
}

#[test]
fn test_c3_wizard_base_url_of_a_non_ollama_provider_does_not_become_the_ollama_url() {
    let w = user("[provider]\nname = \"deepseek\"\nbase_url = \"https://api.deepseek.com\"\n");
    let e = run(Some(&w), None, None, &no_env());
    assert_eq!(e.ollama.base_url.as_deref(), Some("http://localhost:11434"));
}

// -- C4: api_key_env -------------------------------------------------------------------------------

#[test]
fn test_c4_default_variables_supply_the_keys() {
    let (a, o) = (key("sk-ant"), key("sk"));
    let env = env_of(&[("ANTHROPIC_API_KEY", a.clone()), ("OPENAI_API_KEY", o.clone())]);
    let e = run(None, None, None, &env);
    assert_eq!(e.anthropic.api_key.as_deref(), Some(a.as_str()));
    assert_eq!(e.openai.api_key.as_deref(), Some(o.as_str()));
}

#[test]
fn test_c4_api_key_env_names_the_variable_that_holds_the_key() {
    let k = key("sk");
    let u = user("[providers.openai]\napi_key_env = \"MY_TEAM_KEY\"\n[providers.anthropic]\napi_key_env = \"MY_ANT_KEY\"\n");
    let env = env_of(&[("MY_TEAM_KEY", k.clone()), ("MY_ANT_KEY", key("sk-ant"))]);
    let e = run(Some(&u), None, None, &env);
    assert_eq!(e.openai.api_key.as_deref(), Some(k.as_str()));
    assert_eq!(e.openai.api_key_env.as_deref(), Some("MY_TEAM_KEY"));
    assert_eq!(e.anthropic.api_key_env.as_deref(), Some("MY_ANT_KEY"));
}

#[test]
fn test_c4_unset_named_variable_should_not_fall_back_to_the_default_variable() {
    let u = user("[providers.openai]\napi_key_env = \"MY_TEAM_KEY\"\n");
    let env = env_of(&[("OPENAI_API_KEY", key("sk"))]);
    let e = run(Some(&u), None, None, &env);
    assert_eq!(e.openai.api_key, None, "no silent fallback to OPENAI_API_KEY");
    assert_eq!(e.openai.api_key_env.as_deref(), Some("MY_TEAM_KEY"));
}

#[test]
fn test_c4_project_api_key_env_should_be_ignored_with_a_warning() {
    let k = key("sk");
    let p = project("[providers.openai]\napi_key_env = \"EVIL_VAR\"\n");
    let env = env_of(&[("OPENAI_API_KEY", k.clone()), ("EVIL_VAR", "stolen".to_owned())]);
    let e = run(None, Some(&p), None, &env);
    assert_eq!(e.openai.api_key.as_deref(), Some(k.as_str()));
    assert_eq!(e.openai.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    let ws = warnings_for(&e, PROJECT_PATH);
    assert_eq!(ws.len(), 1, "{:?}", e.warnings);
    assert!(ws[0].message.contains("api_key_env"), "{}", ws[0].message);
}

// -- C5: unknown providers / api_key_env under ollama --------------------------------------------------

#[test]
fn test_c5_unknown_provider_table_should_be_a_config_error_naming_the_file() {
    for (path, raw) in [(USER_PATH, "[providers.mistral]\nmodel = \"x\"\n"), (PROJECT_PATH, "[providers.gemini]\nmodel = \"x\"\n")] {
        let l = layer(path, raw);
        let (u, p) = if path == USER_PATH { (Some(&l), None) } else { (None, Some(&l)) };
        let e = resolve(u, p, None, &no_env()).expect_err("unknown provider must be an error");
        assert_eq!(err_path(&e), Path::new(path));
        assert!(err_reason(&e).contains("mistral") || err_reason(&e).contains("gemini"), "{e}");
    }
}

#[test]
fn test_c5_api_key_env_under_ollama_should_be_a_config_error() {
    let u = user("[providers.ollama]\napi_key_env = \"X\"\n");
    let e = resolve(Some(&u), None, None, &no_env()).expect_err("ollama has no key");
    assert_eq!(err_path(&e), Path::new(USER_PATH));
    assert!(err_reason(&e).contains("api_key_env"), "{e}");
}

// -- C6: the wizard [provider] table --------------------------------------------------------------------

#[test]
fn test_c6_wizard_table_should_be_accepted_by_the_strict_schema() {
    let cfg = Config::from_toml_str("[provider]\nname = \"anthropic\"\napi_key = \"k\"\nmodel = \"claude-sonnet-4-5\"\n").expect("wizard file parses");
    let w = cfg.provider.expect("provider table");
    assert_eq!(w.name.as_deref(), Some("anthropic"));
    assert_eq!(w.api_key.as_deref(), Some("k"));
    assert_eq!(w.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(w.base_url, None);
}

#[test]
fn test_c6_unknown_key_in_wizard_table_should_still_be_rejected() {
    let e = ConfigLayer::from_toml(USER_PATH, "[provider]\nname = \"openai\"\nbogus = 1\n").expect_err("strict");
    assert_eq!(err_path(&e), Path::new(USER_PATH));
}

#[test]
fn test_c6_wizard_name_and_model_imply_a_default_model_per_provider() {
    for (name, model, expected) in [
        ("anthropic", "claude-opus-4-5", "anthropic:claude-opus-4-5"),
        ("openai", "gpt-4o", "openai:gpt-4o"),
        ("deepseek", "deepseek-chat", "openai:deepseek-chat"),
        ("openrouter", "openai/gpt-4o", "openai:openai/gpt-4o"),
        ("ollama", "llama3.2", "ollama:llama3.2"),
        ("claude-cli", "claude-sonnet-4-5", "claude-cli:claude-sonnet-4-5"),
    ] {
        let u = user(&format!("[provider]\nname = {name:?}\nmodel = {model:?}\n"));
        let e = run(Some(&u), None, None, &no_env());
        let (m, s) = model_of(&e).unwrap_or_else(|| panic!("{name}: an implied model"));
        assert_eq!(m, expected, "{name}");
        assert_eq!(*s, ModelSource::Wizard { path: PathBuf::from(USER_PATH) }, "{name}");
        assert!(e.warnings.is_empty(), "{name}: {:?}", e.warnings);
    }
}

#[test]
fn test_c6_wizard_without_model_implies_nothing() {
    let u = user("[provider]\nname = \"ollama\"\n");
    assert!(run(Some(&u), None, None, &no_env()).model.is_none());
}

#[test]
fn test_c6_wizard_key_fills_the_key_when_no_variable_is_set_and_the_provider_maps() {
    let (a, o) = (key("sk-ant"), key("sk"));
    let wa = user(&format!("[provider]\nname = \"anthropic\"\napi_key = {a:?}\n"));
    let e = run(Some(&wa), None, None, &no_env());
    assert_eq!(e.anthropic.api_key.as_deref(), Some(a.as_str()));
    assert_eq!(e.openai.api_key, None, "an anthropic wizard key must not become the OpenAI key");

    for name in ["openai", "deepseek", "openrouter"] {
        let wo = user(&format!("[provider]\nname = {name:?}\napi_key = {o:?}\n"));
        let e = run(Some(&wo), None, None, &no_env());
        assert_eq!(e.openai.api_key.as_deref(), Some(o.as_str()), "{name}");
        assert_eq!(e.anthropic.api_key, None, "{name}");
    }
}

#[test]
fn test_c6_environment_variable_beats_the_wizard_key() {
    let (env_key, wiz_key) = (key("sk-env"), key("sk-wiz"));
    let w = user(&format!("[provider]\nname = \"openai\"\napi_key = {wiz_key:?}\n"));
    let env = env_of(&[("OPENAI_API_KEY", env_key.clone())]);
    assert_eq!(run(Some(&w), None, None, &env).openai.api_key.as_deref(), Some(env_key.as_str()));
}

#[test]
fn test_c6_wizard_table_in_the_project_file_is_ignored_with_a_warning() {
    let p = project(&format!(
        "[provider]\nname = \"openai\"\napi_key = {:?}\nmodel = \"evil-model\"\nbase_url = \"https://evil.example\"\n",
        key("sk")
    ));
    let e = run(None, Some(&p), None, &no_env());
    assert!(e.model.is_none(), "no implied model from a project wizard table");
    assert_eq!(e.openai.api_key, None);
    assert_eq!(e.openai.base_url, None);
    assert_eq!(e.openai.default_model, "gpt-4o");
    let ws = warnings_for(&e, PROJECT_PATH);
    assert_eq!(ws.len(), 1, "{:?}", e.warnings);
    assert!(ws[0].message.contains("[provider]") || ws[0].message.contains("provider"), "{}", ws[0].message);
}

// -- C7: session_limit_usd -----------------------------------------------------------------------------

#[test]
fn test_c7_session_limit_from_either_layer() {
    let u = user("[budget]\nsession_limit_usd = 5.0\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).session_limit_usd, Some(5.0));
    let p = project("[budget]\nsession_limit_usd = 2.5\n");
    assert_eq!(run(None, Some(&p), None, &no_env()).session_limit_usd, Some(2.5));
}

#[test]
fn test_c7_effective_limit_is_the_minimum_of_user_and_project() {
    let u = user("[budget]\nsession_limit_usd = 5.0\n");
    let lower = project("[budget]\nsession_limit_usd = 1.0\n");
    let higher = project("[budget]\nsession_limit_usd = 50.0\n");
    assert_eq!(run(Some(&u), Some(&lower), None, &no_env()).session_limit_usd, Some(1.0));
    assert_eq!(
        run(Some(&u), Some(&higher), None, &no_env()).session_limit_usd,
        Some(5.0),
        "a project can only lower the cap"
    );
}

#[test]
fn test_c7_zero_is_a_valid_limit() {
    let u = user("[budget]\nsession_limit_usd = 0.0\n");
    assert_eq!(run(Some(&u), None, None, &no_env()).session_limit_usd, Some(0.0));
}

// -- C8: keys that are parsed but not applied ---------------------------------------------------------------

#[test]
fn test_c8_unapplied_keys_warn_once_each_naming_the_alternative() {
    let u = user(
        r#"
[budget]
daily_limit_usd = 10.0
[sandbox]
allowed_commands = ["ls"]
[[routing]]
task = "file_read"
model = "fast"
[server]
port = 4000
"#,
    );
    let e = run(Some(&u), None, None, &no_env());
    let ws = warnings_for(&e, USER_PATH);
    assert_eq!(ws.len(), 4, "one per key: {:?}", e.warnings);
    let all: String = ws.iter().map(|w| w.message.clone()).collect::<Vec<_>>().join("\n");
    for needle in ["daily_limit_usd", "sandbox", "routing", "server"] {
        assert!(ws.iter().any(|w| w.message.contains(needle)), "no warning for {needle}: {all}");
    }
    assert!(all.contains("permissions.toml") && all.contains("HONE_SANDBOX_"), "sandbox alternative: {all}");
    assert!(all.contains("--model") && all.contains("default_model"), "routing alternative: {all}");
    assert!(all.contains("HONE_BIND") && all.contains("HONE_AUTH_TOKEN"), "server alternative: {all}");
    assert_eq!(e.session_limit_usd, None, "daily limit is not a session limit");
}

#[test]
fn test_c8_project_file_gets_the_same_warnings_with_its_own_path() {
    let p = project("[budget]\ndaily_limit_usd = 1.0\n[[routing]]\ntask = \"t\"\nmodel = \"m\"\n");
    let e = run(None, Some(&p), None, &no_env());
    assert_eq!(warnings_for(&e, PROJECT_PATH).len(), 2, "{:?}", e.warnings);
}

#[test]
fn test_c8_a_clean_file_has_no_warnings() {
    let u = user("default_model = \"openai:gpt-4o\"\n[budget]\nsession_limit_usd = 3.0\n[providers.openai]\nmodel = \"gpt-4o\"\n");
    assert!(run(Some(&u), None, None, &no_env()).warnings.is_empty());
}

// -- C12: errors ---------------------------------------------------------------------------------------------

#[test]
fn test_c12_parse_errors_name_the_file() {
    for (raw, what) in [
        ("this is = = not toml", "syntax"),
        ("totally_unknown = 1", "unknown key"),
        ("[budget]\nsession_limit_usd = \"five\"", "wrong type"),
        ("[providers.openai]\nunknown_field = 1", "unknown provider field"),
    ] {
        let e = ConfigLayer::from_toml(USER_PATH, raw).expect_err(what);
        assert_eq!(err_path(&e), Path::new(USER_PATH), "{what}");
        assert!(!err_reason(&e).is_empty(), "{what}");
    }
}

#[test]
fn test_c12_load_of_a_missing_file_is_none_and_of_a_broken_or_unreadable_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.toml");
    assert!(ConfigLayer::load(&missing).unwrap().is_none());

    let broken = dir.path().join("broken.toml");
    std::fs::write(&broken, "default_model = ").unwrap();
    let e = ConfigLayer::load(&broken).expect_err("broken file");
    assert_eq!(err_path(&e), broken.as_path());

    // A directory where a file is expected: exists but can't be read.
    let e = ConfigLayer::load(dir.path()).expect_err("unreadable");
    assert_eq!(err_path(&e), dir.path());

    let ok = dir.path().join("ok.toml");
    std::fs::write(&ok, "default_model = \"openai:x\"\n").unwrap();
    let l = ConfigLayer::load(&ok).unwrap().expect("a layer");
    assert_eq!(l.path, ok);
}

#[test]
fn test_c12_default_model_without_a_known_prefix_is_an_error_in_either_file() {
    for bad in ["gpt-4o", "mistral:7b", ""] {
        let u = user(&format!("default_model = {bad:?}"));
        let e = resolve(Some(&u), None, None, &no_env()).expect_err(bad);
        assert_eq!(err_path(&e), Path::new(USER_PATH), "{bad}");
        assert!(err_reason(&e).contains("default_model"), "{bad}: {e}");
        let p = project(&format!("default_model = {bad:?}"));
        let e = resolve(None, Some(&p), None, &no_env()).expect_err(bad);
        assert_eq!(err_path(&e), Path::new(PROJECT_PATH), "{bad}");
    }
}

#[test]
fn test_c12_negative_or_non_finite_limits_are_errors() {
    for raw in [
        "[budget]\nsession_limit_usd = -1.0\n",
        "[budget]\nsession_limit_usd = nan\n",
        "[budget]\nsession_limit_usd = inf\n",
        "[budget]\ndaily_limit_usd = -5.0\n",
    ] {
        let u = user(raw);
        let e = resolve(Some(&u), None, None, &no_env()).expect_err(raw);
        assert_eq!(err_path(&e), Path::new(USER_PATH), "{raw}");
        assert!(err_reason(&e).contains("limit"), "{raw}: {e}");
    }
}

#[test]
fn test_c12_unknown_wizard_provider_name_is_an_error() {
    let u = user("[provider]\nname = \"skynet\"\nmodel = \"m\"\n");
    let e = resolve(Some(&u), None, None, &no_env()).expect_err("unknown provider name");
    assert_eq!(err_path(&e), Path::new(USER_PATH));
    assert!(err_reason(&e).contains("skynet"), "{e}");
}

#[test]
fn test_c12_error_in_the_project_layer_names_the_project_file() {
    let u = user("default_model = \"openai:ok\"\n");
    let p = project("default_model = \"nonsense\"\n");
    let e = resolve(Some(&u), Some(&p), None, &no_env()).expect_err("bad project file");
    assert_eq!(err_path(&e), Path::new(PROJECT_PATH));
}

// keep the unused-import lint quiet for types referenced only in the pinned-API docs
#[allow(dead_code)]
fn _api_shape(_: &ProviderSettings) {}
