//! Persistent desktop-app preferences (~/.config/hone/desktop.json).
//!
//! Kept tiny and forward-compatible: unknown fields are preserved on round-
//! trip via `serde_json::Value`, but only the fields we read are typed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMode {
    /// We spawn and supervise our own honed child.
    #[default]
    Sidecar,
    /// We speak HTTP to a daemon the user manages elsewhere.
    External { url: String, token: Option<String> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesktopConfig {
    #[serde(default)]
    pub daemon: Option<DaemonMode>,
}

impl DesktopConfig {
    pub fn load_or_default() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        std::fs::write(path, body)
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("hone").join("desktop.json"))
}
