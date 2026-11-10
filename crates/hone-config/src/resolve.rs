//! Pure resolution of `config.toml` layers into the settings `hone` runs with.
//!
//! Two layers feed [`resolve`]: the user file (trusted) and the project file
//! (untrusted, it ships with the repository). A project file may choose
//! models and lower the budget; it may not set endpoints, key variables or
//! keys, because that would send the user's credentials and code to a host the
//! repository chose. Environment access goes through a lookup function so the
//! resolver stays pure.

use std::path::{Path, PathBuf};

use hone_proto::ModelId;

use crate::policy::{read_checked_text, toml_location};
use crate::{Config, ConfigError, PermissionError, WizardProviderConfig};

const KNOWN_PREFIXES: [&str; 5] = [
    "anthropic:",
    "openai:",
    "ollama:",
    "claude-cli:",
    "gemini-cli:",
];
const CLI_PREFIXES: [&str; 2] = ["claude-cli:", "gemini-cli:"];
const KNOWN_PROVIDERS: [&str; 3] = ["anthropic", "openai", "ollama"];
const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

/// One parsed config file and where it came from.
#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub path: PathBuf,
    pub config: Config,
}

impl ConfigLayer {
    /// Parse `raw` as the file at `path`. A parse error carries the message and
    /// `line:column` only, never a line of the file.
    pub fn from_toml(path: impl Into<PathBuf>, raw: &str) -> Result<ConfigLayer, ConfigError> {
        let path = path.into();
        match toml::from_str::<Config>(raw) {
            Ok(config) => Ok(ConfigLayer { path, config }),
            Err(e) => Err(ConfigError::InvalidField {
                path,
                reason: toml_location(raw, &e),
            }),
        }
    }

    /// Load a file like `permissions.toml`: a regular file (symlinks followed) of
    /// at most 64 KiB and valid UTF-8. `Ok(None)` if it does not exist; anything
    /// else wrong is an error naming the path.
    pub fn load(path: &Path) -> Result<Option<ConfigLayer>, ConfigError> {
        match read_checked_text(path) {
            Ok(Some(raw)) => Self::from_toml(path, &raw).map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(invalid(path, permission_reason(e))),
        }
    }

    /// Load the project layer: `<workdir>/.hone` and `<workdir>/.hone/config.toml`
    /// must not be symlinks (the file comes with the repository), then the same
    /// checks as [`ConfigLayer::load`].
    pub fn load_project(workdir: &Path) -> Result<Option<ConfigLayer>, ConfigError> {
        let dir = workdir.join(".hone");
        let file = dir.join("config.toml");
        for (p, what) in [(&dir, ".hone directory"), (&file, "config file")] {
            match std::fs::symlink_metadata(p) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(invalid(
                        p,
                        format!("the project {what} must not be a symlink"),
                    ));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(invalid(p, format!("cannot read: {e}"))),
            }
        }
        Self::load(&file)
    }
}

fn permission_reason(e: PermissionError) -> String {
    match e {
        PermissionError::UnsafeFile { reason, .. } => reason,
        PermissionError::Toml(m) => m,
        PermissionError::Io(e) => format!("cannot read file: {e}"),
        other => other.to_string(),
    }
}

/// Check the model name (the part after the provider prefix): non-empty, no
/// leading `-`, no whitespace or control characters. Guards against a name being
/// read as a flag by an agent CLI.
pub fn validate_model_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the model name is empty".to_owned());
    }
    if name.starts_with('-') {
        return Err("the model name must not start with '-'".to_owned());
    }
    if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the model name must not contain whitespace or control characters".to_owned());
    }
    Ok(())
}

/// Where an explicit model choice came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    Cli,
    DefaultModel { path: PathBuf },
    Wizard { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub model: ModelId,
    pub source: ModelSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSettings {
    /// The resolved key (`None` = none available).
    pub api_key: Option<String>,
    /// The variable consulted for the key (user override or default).
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    /// The model used when auto-detection picks this provider.
    pub default_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Effective {
    /// `None` means auto-detect.
    pub model: Option<ModelChoice>,
    pub anthropic: ProviderSettings,
    /// Also serves deepseek / openrouter.
    pub openai: ProviderSettings,
    pub ollama: ProviderSettings,
    pub session_limit_usd: Option<f64>,
    pub warnings: Vec<Warning>,
    /// The routed model per complexity tier (`[[routing]]`).
    pub routing: TierRouting,
    /// `fallback_models`, filtered and deduplicated.
    pub fallback_models: Vec<ModelId>,
    /// The file `fallback_models` came from.
    pub fallback_path: Option<PathBuf>,
}

fn invalid(path: &Path, reason: impl Into<String>) -> ConfigError {
    ConfigError::InvalidField {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Which resolved provider a wizard `name` fills.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WizardTarget {
    Anthropic,
    OpenAi,
    Ollama,
    ClaudeCli,
}

fn wizard_target(name: &str) -> Option<WizardTarget> {
    match name {
        "anthropic" => Some(WizardTarget::Anthropic),
        "openai" | "deepseek" | "openrouter" => Some(WizardTarget::OpenAi),
        "ollama" => Some(WizardTarget::Ollama),
        "claude-cli" => Some(WizardTarget::ClaudeCli),
        _ => None,
    }
}

/// Reject what the schema cannot: unknown provider tables, `api_key_env` under
/// ollama, a `default_model` without a known prefix, bad limits, unknown wizard names.
fn validate(layer: &ConfigLayer) -> Result<(), ConfigError> {
    let (path, cfg) = (&layer.path, &layer.config);
    for (name, p) in &cfg.providers {
        if !KNOWN_PROVIDERS.contains(&name.as_str()) {
            return Err(invalid(
                path,
                format!(
                    "unknown provider '{name}' in [providers] (expected anthropic, openai or ollama)"
                ),
            ));
        }
        if name == "ollama" && p.api_key_env.is_some() {
            return Err(invalid(
                path,
                "[providers.ollama].api_key_env is not valid: ollama has no API key",
            ));
        }
    }
    if let Some(m) = &cfg.default_model {
        if !KNOWN_PREFIXES.iter().any(|p| m.as_str().starts_with(p)) {
            return Err(invalid(
                path,
                format!(
                    "default_model '{m}' must start with a provider prefix ({})",
                    KNOWN_PREFIXES.join(" ")
                ),
            ));
        }
    }
    if let Some(m) = &cfg.default_model {
        let name = KNOWN_PREFIXES
            .iter()
            .find_map(|p| m.as_str().strip_prefix(p))
            .unwrap_or_default();
        if let Err(why) = validate_model_name(name) {
            return Err(invalid(path, format!("default_model '{m}': {why}")));
        }
    }
    for (prov, p) in &cfg.providers {
        if let Some(model) = &p.model {
            if let Err(why) = validate_model_name(model) {
                return Err(invalid(path, format!("[providers.{prov}].model: {why}")));
            }
        }
    }
    if let Some(model) = cfg.provider.as_ref().and_then(|w| w.model.as_deref()) {
        if let Err(why) = validate_model_name(model) {
            return Err(invalid(path, format!("[provider].model: {why}")));
        }
    }
    // [[routing]] models are validated whatever their task: a bad model is an
    // error even when the task is unknown.
    for entry in &cfg.routing {
        if let Err(why) = check_api_or_cli_model(&entry.model) {
            return Err(invalid(
                path,
                format!("[[routing]] model '{}': {why}", entry.model),
            ));
        }
    }
    for m in &cfg.fallback_models {
        if let Err(why) = check_api_or_cli_model(m.as_str()) {
            return Err(invalid(path, format!("fallback_models '{m}': {why}")));
        }
    }
    if let Some(b) = &cfg.budget {
        for (key, v) in [
            ("session_limit_usd", b.session_limit_usd),
            ("daily_limit_usd", b.daily_limit_usd),
        ] {
            if let Some(v) = v {
                if !v.is_finite() || v < 0.0 {
                    return Err(invalid(
                        path,
                        format!("[budget].{key} must be a finite, non-negative limit (got {v})"),
                    ));
                }
            }
        }
    }
    if let Some(name) = cfg.provider.as_ref().and_then(|w| w.name.as_deref()) {
        if wizard_target(name).is_none() {
            return Err(invalid(
                path,
                format!("unknown provider name '{name}' in [provider]"),
            ));
        }
    }
    Ok(())
}

fn warn(out: &mut Vec<Warning>, path: &Path, message: impl Into<String>) {
    out.push(Warning {
        path: path.to_path_buf(),
        message: message.into(),
    });
}

/// Keys that are parsed but not applied, plus (project only) trusted-only keys.
fn collect_warnings(layer: &ConfigLayer, trusted: bool, out: &mut Vec<Warning>) {
    let (path, cfg) = (&layer.path, &layer.config);
    if cfg
        .budget
        .as_ref()
        .is_some_and(|b| b.daily_limit_usd.is_some())
    {
        warn(
            out,
            path,
            "[budget].daily_limit_usd is not applied (it needs spend persisted across \
             sessions); use [budget].session_limit_usd or /budget",
        );
    }
    if cfg.sandbox.is_some() {
        warn(
            out,
            path,
            "[sandbox] is not applied; use permissions.toml and the HONE_SANDBOX_* \
             environment variables",
        );
    }
    if cfg.server.is_some() {
        warn(
            out,
            path,
            "[server] is not applied; use honed's port argument, HONE_BIND and HONE_AUTH_TOKEN",
        );
    }
    if trusted {
        return;
    }
    if let Some(m) = &cfg.default_model {
        if CLI_PREFIXES.iter().any(|p| m.as_str().starts_with(p)) {
            warn(
                out,
                path,
                format!(
                    "default_model '{m}' is ignored in a project config (an agent CLI runs \
                     its own tools outside hone's policy; choose it in the user config or \
                     with --model)"
                ),
            );
        }
    }
    let mut names: Vec<&String> = cfg.providers.keys().collect();
    names.sort();
    for name in names {
        let p = &cfg.providers[name];
        if p.base_url.is_some() {
            warn(
                out,
                path,
                format!(
                    "[providers.{name}].base_url is ignored in a project config \
                     (set it in the user config)"
                ),
            );
        }
        if p.api_key_env.is_some() {
            warn(
                out,
                path,
                format!(
                    "[providers.{name}].api_key_env is ignored in a project config \
                     (set it in the user config)"
                ),
            );
        }
    }
    if cfg.provider.is_some() {
        warn(
            out,
            path,
            "[provider] is ignored in a project config (it holds keys and endpoints; \
             set it in the user config)",
        );
    }
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// Complexity tier of a turn, as chosen by the complexity classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Simple,
    Medium,
    Complex,
}

impl Tier {
    fn from_task(task: &str) -> Option<Tier> {
        match task {
            "simple" => Some(Tier::Simple),
            "medium" => Some(Tier::Medium),
            "complex" => Some(Tier::Complex),
            _ => None,
        }
    }

    /// The tier's name as used in `[[routing]]` and messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Simple => "simple",
            Tier::Medium => "medium",
            Tier::Complex => "complex",
        }
    }
}

/// A model chosen by `[[routing]]`, with the file the entry came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedModel {
    pub model: ModelId,
    pub path: PathBuf,
}

/// The routed model per tier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierRouting {
    simple: Option<RoutedModel>,
    medium: Option<RoutedModel>,
    complex: Option<RoutedModel>,
}

impl TierRouting {
    pub fn get(&self, tier: Tier) -> Option<&RoutedModel> {
        match tier {
            Tier::Simple => self.simple.as_ref(),
            Tier::Medium => self.medium.as_ref(),
            Tier::Complex => self.complex.as_ref(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.simple.is_none() && self.medium.is_none() && self.complex.is_none()
    }

    fn slot(&mut self, tier: Tier) -> &mut Option<RoutedModel> {
        match tier {
            Tier::Simple => &mut self.simple,
            Tier::Medium => &mut self.medium,
            Tier::Complex => &mut self.complex,
        }
    }
}

/// A routed or fallback model must have a known provider prefix and a valid name.
fn check_api_or_cli_model(model: &str) -> Result<(), String> {
    match KNOWN_PREFIXES.iter().find_map(|p| model.strip_prefix(p)) {
        None => Err(format!(
            "must start with a provider prefix ({})",
            KNOWN_PREFIXES.join(" ")
        )),
        Some(name) => validate_model_name(name),
    }
}

fn is_cli_model(model: &str) -> bool {
    CLI_PREFIXES.iter().any(|p| model.starts_with(p))
}

/// The accepted `[[routing]]` entries and `fallback_models` of one file, with
/// a warning for each entry that is ignored.
fn routing_of(
    layer: &ConfigLayer,
    trusted: bool,
    out: &mut Vec<Warning>,
) -> (TierRouting, Vec<ModelId>) {
    let (path, cfg) = (&layer.path, &layer.config);
    let mut routing = TierRouting::default();
    for entry in &cfg.routing {
        let Some(tier) = Tier::from_task(&entry.task) else {
            warn(
                out,
                path,
                format!(
                    "[[routing]] task '{}' is not a complexity tier (simple, medium, complex); \
                     entry ignored",
                    entry.task
                ),
            );
            continue;
        };
        if is_cli_model(&entry.model) {
            warn(
                out,
                path,
                format!(
                    "[[routing]] model '{}' ignored: an agent CLI (claude-cli, gemini-cli) \
                     cannot be routed to",
                    entry.model
                ),
            );
            continue;
        }
        let slot = routing.slot(tier);
        if slot.is_some() {
            warn(
                out,
                path,
                format!(
                    "[[routing]] has more than one entry for task '{}'; the first one is used",
                    entry.task
                ),
            );
            continue;
        }
        *slot = Some(RoutedModel {
            model: ModelId::new(entry.model.clone()),
            path: path.clone(),
        });
    }

    // A fallback model is used without notice when the chosen one fails, so a
    // repository must not choose it: only the user file's list applies. The
    // project's list was validated; it is ignored with one warning.
    if !trusted {
        if !cfg.fallback_models.is_empty() {
            warn(
                out,
                path,
                "fallback_models is ignored in a project config (a fallback is used without \
                 notice, so a repository must not choose it; set it in the user config)",
            );
        }
        return (routing, Vec::new());
    }

    let mut fallback: Vec<ModelId> = Vec::new();
    for m in &cfg.fallback_models {
        if is_cli_model(m.as_str()) {
            warn(
                out,
                path,
                format!(
                    "fallback_models '{m}' ignored: an agent CLI (claude-cli, gemini-cli) \
                     cannot be a fallback"
                ),
            );
        } else if !fallback.contains(m) {
            fallback.push(m.clone());
        }
    }
    (routing, fallback)
}

/// The model one turn runs on, and the file whose `[[routing]]` chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnModel {
    pub model: ModelId,
    pub routed_from: Option<PathBuf>,
}

/// Pick a turn's model: the pinned model if any; else the `[[routing]]` model for
/// `tier` if its provider is `available`; else `default`.
pub fn turn_model(
    eff: &Effective,
    tier: Tier,
    pinned: Option<&ModelId>,
    default: &ModelId,
    available: &dyn Fn(&ModelId) -> bool,
) -> TurnModel {
    if let Some(p) = pinned {
        return TurnModel {
            model: p.clone(),
            routed_from: None,
        };
    }
    if let Some(r) = eff.routing.get(tier) {
        if available(&r.model) {
            return TurnModel {
                model: r.model.clone(),
                routed_from: Some(r.path.clone()),
            };
        }
    }
    TurnModel {
        model: default.clone(),
        routed_from: None,
    }
}

/// Validate a `/model` switch: an API-prefixed model with a valid name whose
/// provider is `available`. The error is a message for the user.
pub fn check_model_switch(
    model: &str,
    available: &dyn Fn(&ModelId) -> bool,
) -> Result<ModelId, String> {
    if is_cli_model(model) {
        return Err(format!(
            "cannot switch to '{model}': agent CLI models (claude-cli, gemini-cli) run their \
             own tools and cannot be selected mid-session"
        ));
    }
    let prefix = KNOWN_PREFIXES
        .iter()
        .find(|p| model.starts_with(*p) && !CLI_PREFIXES.contains(p));
    let Some(prefix) = prefix else {
        return Err(format!(
            "invalid model '{model}': it must start with anthropic:, openai: or ollama:"
        ));
    };
    validate_model_name(&model[prefix.len()..])
        .map_err(|why| format!("invalid model '{model}': {why}"))?;
    let id = ModelId::new(model);
    if !available(&id) {
        let provider = prefix.trim_end_matches(':');
        return Err(format!(
            "cannot switch to '{model}': no {provider} provider is configured (missing API key)"
        ));
    }
    Ok(id)
}

/// Resolve the effective settings. `cli_model` is `--model`, already known to be
/// non-empty. Every error is `ConfigError::InvalidField { path, reason }`.
pub fn resolve(
    user: Option<&ConfigLayer>,
    project: Option<&ConfigLayer>,
    cli_model: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Effective, ConfigError> {
    for layer in [user, project].into_iter().flatten() {
        validate(layer)?;
    }

    let mut warnings = Vec::new();
    if let Some(l) = user {
        collect_warnings(l, true, &mut warnings);
    }
    if let Some(l) = project {
        collect_warnings(l, false, &mut warnings);
    }

    // [[routing]] and fallback_models: per task / as a whole, the project file's
    // entry is used before the user file's.
    let mut routing = TierRouting::default();
    let mut fallback_models: Vec<ModelId> = Vec::new();
    let mut fallback_path: Option<PathBuf> = None;
    let mut layers_routing: Vec<(&ConfigLayer, TierRouting, Vec<ModelId>)> = Vec::new();
    for (l, trusted) in [(user, true), (project, false)]
        .into_iter()
        .filter_map(|(l, t)| l.map(|l| (l, t)))
    {
        let (r, f) = routing_of(l, trusted, &mut warnings);
        layers_routing.push((l, r, f));
    }
    for (l, r, f) in layers_routing.iter().rev() {
        for tier in [Tier::Simple, Tier::Medium, Tier::Complex] {
            if routing.get(tier).is_none() {
                if let Some(entry) = r.get(tier) {
                    *routing.slot(tier) = Some(entry.clone());
                }
            }
        }
        if fallback_models.is_empty() && !f.is_empty() {
            fallback_models = f.clone();
            fallback_path = Some(l.path.clone());
        }
    }

    // The wizard table is only honoured from the trusted user file.
    let wizard: Option<&WizardProviderConfig> = user.and_then(|l| l.config.provider.as_ref());
    let wizard_for =
        |t: WizardTarget| wizard.filter(|w| w.name.as_deref().and_then(wizard_target) == Some(t));

    // -- model choice ---------------------------------------------------------------------
    let model = if let Some(m) = cli_model {
        Some(ModelChoice {
            model: ModelId::new(m),
            source: ModelSource::Cli,
        })
    } else if let Some((l, m)) = [(project, false), (user, true)]
        .into_iter()
        .filter_map(|(l, trusted)| l.map(|l| (l, trusted)))
        .find_map(|(l, trusted)| {
            l.config
                .default_model
                .clone()
                // Choosing an agent CLI is trusted-only: a project file's choice falls through.
                .filter(|m| trusted || !CLI_PREFIXES.iter().any(|p| m.as_str().starts_with(p)))
                .map(|m| (l, m))
        })
    {
        Some(ModelChoice {
            model: m,
            source: ModelSource::DefaultModel {
                path: l.path.clone(),
            },
        })
    } else if let (Some(l), Some(w)) = (user, wizard) {
        let prefix = match w.name.as_deref().and_then(wizard_target) {
            Some(WizardTarget::Anthropic) => Some("anthropic:"),
            Some(WizardTarget::OpenAi) => Some("openai:"),
            Some(WizardTarget::Ollama) => Some("ollama:"),
            Some(WizardTarget::ClaudeCli) => Some("claude-cli:"),
            None => None,
        };
        match (prefix, non_empty(w.model.clone())) {
            (Some(p), Some(m)) => Some(ModelChoice {
                model: ModelId::new(format!("{p}{m}")),
                source: ModelSource::Wizard {
                    path: l.path.clone(),
                },
            }),
            _ => None,
        }
    } else {
        None
    };

    // -- provider settings ----------------------------------------------------------------
    let user_provider = |name: &str| user.and_then(|l| l.config.providers.get(name));
    // A project may only choose models.
    let model_from_files = |name: &str| {
        [project, user]
            .into_iter()
            .flatten()
            .find_map(|l| l.config.providers.get(name).and_then(|p| p.model.clone()))
    };

    let keyed = |name: &str, default_var: &str, target: WizardTarget| {
        let var = user_provider(name)
            .and_then(|p| p.api_key_env.clone())
            .unwrap_or_else(|| default_var.to_owned());
        // An unset named variable gives no environment key; the wizard key may still apply.
        let api_key = non_empty(env(&var))
            .or_else(|| wizard_for(target).and_then(|w| non_empty(w.api_key.clone())));
        (var, api_key)
    };

    let (a_var, a_key) = keyed("anthropic", "ANTHROPIC_API_KEY", WizardTarget::Anthropic);
    let anthropic = ProviderSettings {
        api_key: a_key,
        api_key_env: Some(a_var),
        base_url: user_provider("anthropic").and_then(|p| p.base_url.clone()),
        default_model: model_from_files("anthropic")
            .or_else(|| wizard_for(WizardTarget::Anthropic).and_then(|w| w.model.clone()))
            .unwrap_or_else(|| "claude-sonnet-4-5".to_owned()),
    };

    let (o_var, o_key) = keyed("openai", "OPENAI_API_KEY", WizardTarget::OpenAi);
    let openai = ProviderSettings {
        api_key: o_key,
        api_key_env: Some(o_var),
        base_url: non_empty(env("HONE_OPENAI_BASE_URL"))
            .or_else(|| user_provider("openai").and_then(|p| p.base_url.clone()))
            .or_else(|| wizard_for(WizardTarget::OpenAi).and_then(|w| w.base_url.clone())),
        default_model: non_empty(env("HONE_OPENAI_MODEL"))
            .or_else(|| model_from_files("openai"))
            .or_else(|| wizard_for(WizardTarget::OpenAi).and_then(|w| w.model.clone()))
            .unwrap_or_else(|| "gpt-4o".to_owned()),
    };

    let ollama = ProviderSettings {
        api_key: None,
        api_key_env: None,
        base_url: Some(
            user_provider("ollama")
                .and_then(|p| p.base_url.clone())
                .or_else(|| wizard_for(WizardTarget::Ollama).and_then(|w| w.base_url.clone()))
                .unwrap_or_else(|| OLLAMA_DEFAULT_URL.to_owned()),
        ),
        default_model: model_from_files("ollama")
            .or_else(|| wizard_for(WizardTarget::Ollama).and_then(|w| w.model.clone()))
            .unwrap_or_else(|| "llama3.2".to_owned()),
    };

    // -- budget: a project can only lower the cap -------------------------------------------
    let limit = |l: Option<&ConfigLayer>| {
        l.and_then(|l| l.config.budget.as_ref())
            .and_then(|b| b.session_limit_usd)
    };
    let session_limit_usd = match (limit(user), limit(project)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    Ok(Effective {
        model,
        anthropic,
        openai,
        ollama,
        session_limit_usd,
        warnings,
        routing,
        fallback_models,
        fallback_path,
    })
}
