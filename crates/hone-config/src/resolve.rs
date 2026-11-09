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

use crate::{Config, ConfigError, WizardProviderConfig};

const KNOWN_PREFIXES: [&str; 5] = [
    "anthropic:",
    "openai:",
    "ollama:",
    "claude-cli:",
    "gemini-cli:",
];
const KNOWN_PROVIDERS: [&str; 3] = ["anthropic", "openai", "ollama"];
const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

/// One parsed config file and where it came from.
#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub path: PathBuf,
    pub config: Config,
}

impl ConfigLayer {
    /// Parse `raw` as the file at `path`.
    pub fn from_toml(path: impl Into<PathBuf>, raw: &str) -> Result<ConfigLayer, ConfigError> {
        let path = path.into();
        match toml::from_str::<Config>(raw) {
            Ok(config) => Ok(ConfigLayer { path, config }),
            Err(e) => Err(ConfigError::InvalidField {
                path,
                reason: e.to_string(),
            }),
        }
    }

    /// `Ok(None)` if the file does not exist; unreadable or unparsable files
    /// are an error naming the path.
    pub fn load(path: &Path) -> Result<Option<ConfigLayer>, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::from_toml(path, &raw).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ConfigError::InvalidField {
                path: path.to_path_buf(),
                reason: format!("cannot read file: {e}"),
            }),
        }
    }
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
    if !cfg.routing.is_empty() {
        warn(
            out,
            path,
            "[[routing]] is not applied; use --model or default_model",
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
    } else if let Some((l, m)) = [project, user]
        .into_iter()
        .flatten()
        .find_map(|l| l.config.default_model.clone().map(|m| (l, m)))
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
    })
}
