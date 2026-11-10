//! Loading and applying `config.toml`: one strict read of the user and project
//! files, resolved by [`hone_config::resolve`], with errors and warnings
//! reported the way `permissions.toml` problems are.

use std::path::PathBuf;

use hone_config::ConfigError;
use hone_config::resolve::{ConfigLayer, Effective, ModelChoice, ModelSource, resolve};

/// Path of the (trusted) user file.
fn user_config_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/hone/config.toml"))
}

fn workdir() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// Read both files and resolve them with the process environment.
///
/// The project file comes with the repository, so it is loaded without
/// following symlinks (see [`ConfigLayer::load_project`]).
pub fn load_effective(cli_model: Option<&str>) -> Result<Effective, ConfigError> {
    let user = match user_config_path() {
        Some(p) => ConfigLayer::load(&p)?,
        None => None,
    };
    let project = ConfigLayer::load_project(&workdir())?;
    resolve(user.as_ref(), project.as_ref(), cli_model, &|k| {
        std::env::var(k).ok()
    })
}

/// Resolve again as if the project file had no `default_model`: the next
/// choice is the user's `default_model`, then the wizard model, else none.
/// Used when the repository's choice cannot be built. Warnings were already
/// printed by the first load, so none are printed here.
pub fn load_effective_without_project_model() -> Effective {
    let result = (|| {
        let user = match user_config_path() {
            Some(p) => ConfigLayer::load(&p)?,
            None => None,
        };
        let project = ConfigLayer::load_project(&workdir())?.map(|mut l| {
            l.config.default_model = None;
            l
        });
        resolve(user.as_ref(), project.as_ref(), None, &|k| {
            std::env::var(k).ok()
        })
    })();
    match result {
        Ok(eff) => eff,
        Err(e) => {
            eprintln!("{}", format_config_error(&e));
            std::process::exit(2);
        }
    }
}

/// True when the model choice came from the repository's `.hone/config.toml`.
pub fn is_project_sourced(choice: &ModelChoice) -> bool {
    matches!(
        &choice.source,
        ModelSource::DefaultModel { path } if *path == workdir().join(".hone/config.toml")
    )
}

/// Tell the user (stderr, once) that the repository chose the model.
pub fn announce_project_model(choice: &ModelChoice) {
    if let ModelSource::DefaultModel { path } = &choice.source {
        if is_project_sourced(choice) {
            eprintln!(
                "hone: using model {} (from {})",
                choice.model,
                path.display()
            );
        }
    }
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
