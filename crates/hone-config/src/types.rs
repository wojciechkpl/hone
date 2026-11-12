use std::collections::HashMap;
use std::path::{Path, PathBuf};

use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use hone_proto::ModelId;
use serde::{Deserialize, Serialize};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse config: {0}")]
    Parse(String),

    #[error("Config extraction failed: {0}")]
    Figment(Box<figment::Error>),

    #[error("invalid config at {path}: {reason}")]
    InvalidField { path: PathBuf, reason: String },
}

impl From<figment::Error> for ConfigError {
    fn from(e: figment::Error) -> Self {
        ConfigError::Figment(Box::new(e))
    }
}

// ── SandboxMode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SandboxMode {
    /// User must approve every action.
    #[default]
    Manual,
    /// Low-risk actions are allowed automatically; high-risk require approval.
    Supervised,
    /// All actions are executed without prompting.
    Autonomous,
}

// ── Sub-configs ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Name of the environment variable that holds the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Custom base URL for the provider's API.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Default model name to use for this provider.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    #[serde(default)]
    pub mode: SandboxMode,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub allowed_network_domains: Vec<String>,
    #[serde(default)]
    pub protected_paths: Vec<PathBuf>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Manual,
            allowed_commands: Vec::new(),
            allowed_network_domains: Vec::new(),
            protected_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingRule {
    /// Task identifier (e.g. "file_read", "multi_file_edit").
    pub task: String,
    /// Model config name to use for this task.
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    #[serde(default)]
    pub daily_limit_usd: Option<f64>,
    #[serde(default)]
    pub session_limit_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "ServerConfig::default_host")]
    pub host: String,
    #[serde(default = "ServerConfig::default_port")]
    pub port: u16,
    #[serde(default)]
    pub auth_token: Option<String>,
}

impl ServerConfig {
    fn default_host() -> String {
        "127.0.0.1".to_owned()
    }

    fn default_port() -> u16 {
        3117
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            auth_token: None,
        }
    }
}

/// The `[provider]` table `hone configure` writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WizardProviderConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

// ── Config ────────────────────────────────────────────────────────────────────

/// Top-level Hone configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Per-provider settings, keyed by provider name (e.g. "anthropic").
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Default model identifier used when no routing rule matches.
    #[serde(default)]
    pub default_model: Option<ModelId>,

    /// Sandbox policy settings. Parsed but not applied (see `resolve`); an
    /// `Option` so a file that restates the defaults still gets the warning.
    #[serde(default)]
    pub sandbox: Option<SandboxConfig>,

    /// Model routing rules (ordered; first match wins).
    #[serde(default)]
    pub routing: Vec<RoutingRule>,

    /// Optional cost caps.
    #[serde(default)]
    pub budget: Option<BudgetConfig>,

    /// Daemon/server settings. Parsed but not applied (see `resolve`).
    #[serde(default)]
    pub server: Option<ServerConfig>,

    /// Models to try, in order, when the session's model fails (see `resolve`).
    #[serde(default)]
    pub fallback_models: Vec<ModelId>,

    /// The `[provider]` table written by `hone configure`.
    #[serde(default)]
    pub provider: Option<WizardProviderConfig>,

    /// `[pricing."provider:model"]` tables: the user's own per-million-token prices.
    #[serde(default)]
    pub pricing: std::collections::BTreeMap<String, PricingTableEntry>,
}

/// One `[pricing."provider:model"]` table.
///
/// `input_per_million` and `output_per_million` are required; a missing one is
/// reported by key name (the parser's own message would cut it off).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PricingTableEntry {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: Option<f64>,
    pub cache_creation_per_million: Option<f64>,
}

impl<'de> Deserialize<'de> for PricingTableEntry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            input_per_million: Option<f64>,
            output_per_million: Option<f64>,
            cache_read_per_million: Option<f64>,
            cache_creation_per_million: Option<f64>,
        }
        let raw = Raw::deserialize(d)?;
        let need = |v: Option<f64>, key: &str| {
            v.ok_or_else(|| serde::de::Error::custom(format!("a pricing entry needs {key}")))
        };
        Ok(Self {
            input_per_million: need(raw.input_per_million, "input_per_million")?,
            output_per_million: need(raw.output_per_million, "output_per_million")?,
            cache_read_per_million: raw.cache_read_per_million,
            cache_creation_per_million: raw.cache_creation_per_million,
        })
    }
}

impl Config {
    /// Parse a `Config` from a raw TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    /// Load a config by merging global then project layers.
    ///
    /// Priority (highest wins): project file > global file > built-in defaults.
    pub fn load(
        global_path: Option<&Path>,
        project_path: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        // Pre-validate each file with toml::from_str so that parse errors are
        // wrapped with the originating file path before handing off to Figment.
        for path in [global_path, project_path].into_iter().flatten() {
            let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            toml::from_str::<Config>(&raw).map_err(|e| ConfigError::InvalidField {
                path: path.to_path_buf(),
                reason: e.to_string(),
            })?;
        }

        let mut figment = Figment::from(Serialized::defaults(Config::default()));

        if let Some(path) = global_path {
            figment = figment.merge(Toml::file(path));
        }
        if let Some(path) = project_path {
            figment = figment.merge(Toml::file(path));
        }

        let cfg: Config = figment.extract()?;
        Ok(cfg)
    }

    /// Load a config by merging a global TOML string then a project TOML string.
    ///
    /// Useful in tests and when configs are sourced from strings rather than files.
    /// Priority order (highest first): project > global > built-in defaults.
    pub fn load_from_str(global_toml: &str, project_toml: &str) -> Result<Self, ConfigError> {
        // Validate each layer through serde so `deny_unknown_fields` is enforced
        // before handing the raw strings to Figment's sparse TOML provider.
        toml::from_str::<Config>(global_toml)
            .map_err(|e| ConfigError::Parse(format!("global config: {e}")))?;
        toml::from_str::<Config>(project_toml)
            .map_err(|e| ConfigError::Parse(format!("project config: {e}")))?;

        // Toml::string() is a *sparse* provider — it only contributes keys
        // that are actually present in the string, so defaults from the
        // built-in layer are not overwritten by absent project keys.
        // `.merge()` gives the right-hand provider *higher* priority:
        //   defaults < global < project
        let cfg: Config = Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::string(global_toml))
            .merge(Toml::string(project_toml))
            .extract()?;

        Ok(cfg)
    }
}
