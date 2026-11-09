//! Loading and applying `config.toml`: one strict read of the user and project
//! files, resolved by [`hone_config::resolve`], with errors and warnings
//! reported the way `permissions.toml` problems are.

use std::path::PathBuf;

use hone_config::ConfigError;
use hone_config::resolve::{ConfigLayer, Effective, resolve};

/// Paths of the (trusted) user file and the (untrusted) project file.
fn config_paths() -> (Option<PathBuf>, PathBuf) {
    let user = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/hone/config.toml"));
    let cwd = std::env::current_dir().unwrap_or_default();
    (user, cwd.join(".hone/config.toml"))
}

/// Read both files and resolve them with the process environment.
pub fn load_effective(cli_model: Option<&str>) -> Result<Effective, ConfigError> {
    let (user_path, project_path) = config_paths();
    let user = match user_path {
        Some(p) => ConfigLayer::load(&p)?,
        None => None,
    };
    let project = ConfigLayer::load(&project_path)?;
    resolve(user.as_ref(), project.as_ref(), cli_model, &|k| {
        std::env::var(k).ok()
    })
}

/// Format a config error as `hone: config error in <path>: <reason>`.
pub fn format_config_error(e: &ConfigError) -> String {
    match e {
        ConfigError::InvalidField { path, reason } => {
            format!("hone: config error in {}: {reason}", path.display())
        }
        other => format!("hone: config error: {other}"),
    }
}

/// For commands that start a model: load the config, print its warnings once
/// to stderr, and exit 2 on any error (like a broken `permissions.toml`).
pub fn load_effective_or_exit(cli_model: Option<&str>) -> Effective {
    match load_effective(cli_model) {
        Ok(eff) => {
            for w in &eff.warnings {
                eprintln!("hone: warning: {}: {}", w.path.display(), w.message);
            }
            eff
        }
        Err(e) => {
            eprintln!("{}", format_config_error(&e));
            std::process::exit(2);
        }
    }
}
