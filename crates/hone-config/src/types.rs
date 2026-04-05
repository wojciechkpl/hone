use std::collections::HashMap;
use std::path::{Path, PathBuf};

use figment::{
    providers::{Format, Serialized, Toml},
    Figment,
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
    Figment(#[from] figment::Error),
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

// ── Config ────────────────────────────────────────────────────────────────────

/// Top-level Hone configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Per-provider settings, keyed by provider name (e.g. "anthropic").
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Default model identifier used when no routing rule matches.
    #[serde(default)]
    pub default_model: Option<ModelId>,

    /// Sandbox policy settings.
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Model routing rules (ordered; first match wins).
    #[serde(default)]
    pub routing: Vec<RoutingRule>,

    /// Optional cost caps.
    #[serde(default)]
    pub budget: Option<BudgetConfig>,

    /// Daemon/server settings.
    #[serde(default)]
    pub server: ServerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: HashMap::new(),
            default_model: None,
            sandbox: SandboxConfig::default(),
            routing: Vec::new(),
            budget: None,
            server: ServerConfig::default(),
        }
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
