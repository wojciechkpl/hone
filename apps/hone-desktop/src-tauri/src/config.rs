//! Persistent desktop-app preferences (~/.config/hone/desktop.json).
//!
//! Kept tiny and forward-compatible: unknown fields are preserved on round-
//! trip via `serde_json::Value`, but only the fields we read are typed.
//!
//! Phase 5A multi-daemon: a config now carries a *list* of daemon
//! profiles plus an `active` pointer. The legacy single-daemon shape
//! (`{ daemon: <DaemonMode> }`) is migrated transparently on load so old
//! configs keep working.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One configured daemon — either a managed sidecar or an HTTP target.
///
/// Internally tagged so the flattened representation in [`DaemonProfile`]
/// stays predictable on the wire: every profile carries `{ name, kind,
/// ...variant fields }`. The frontend keys off `kind` to render.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonMode {
    /// We spawn and supervise our own honed child.
    #[default]
    Sidecar,
    /// We speak HTTP to a daemon the user manages elsewhere.
    External { url: String, token: Option<String> },
}

/// One named daemon profile in the multi-daemon list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonProfile {
    pub name: String,
    #[serde(flatten)]
    pub mode: DaemonMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// Legacy single-daemon field (kept for read-time migration; never
    /// written by the new code path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonMode>,
    /// Phase 5A: list of named daemon profiles. Empty until the user
    /// adds at least one.
    #[serde(default)]
    pub daemons: Vec<DaemonProfile>,
    /// Name of the currently-active profile. None means "no daemon yet
    /// — show the wizard."
    #[serde(default)]
    pub active: Option<String>,
}

/// Default name assigned to the migrated legacy sidecar entry.
pub const LEGACY_SIDECAR_NAME: &str = "local sidecar";

impl DesktopConfig {
    pub fn load_or_default() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        let mut cfg: DesktopConfig = serde_json::from_str(&raw).unwrap_or_default();
        cfg.migrate_legacy();
        cfg
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

    /// Move a legacy `daemon: <DaemonMode>` field into the new
    /// `daemons` list under [`LEGACY_SIDECAR_NAME`] (sidecar) or
    /// "external" (URL-based). Idempotent — safe to call repeatedly.
    pub fn migrate_legacy(&mut self) {
        if let Some(legacy) = self.daemon.take() {
            let name = match &legacy {
                DaemonMode::Sidecar => LEGACY_SIDECAR_NAME.to_string(),
                DaemonMode::External { .. } => "external".to_string(),
            };
            if !self.daemons.iter().any(|p| p.name == name) {
                self.daemons.push(DaemonProfile { name: name.clone(), mode: legacy });
                if self.active.is_none() {
                    self.active = Some(name);
                }
            }
        }
    }

    /// Return the active profile, falling back to the first one if the
    /// pointer is stale.
    pub fn active_profile(&self) -> Option<&DaemonProfile> {
        self.active
            .as_deref()
            .and_then(|n| self.daemons.iter().find(|p| p.name == n))
            .or_else(|| self.daemons.first())
    }

    /// Add a new profile. Errors if `name` is empty or already exists.
    pub fn add_profile(&mut self, profile: DaemonProfile) -> Result<(), String> {
        if profile.name.trim().is_empty() {
            return Err("profile name must not be empty".into());
        }
        if self.daemons.iter().any(|p| p.name == profile.name) {
            return Err(format!("profile '{}' already exists", profile.name));
        }
        let name = profile.name.clone();
        self.daemons.push(profile);
        if self.active.is_none() {
            self.active = Some(name);
        }
        Ok(())
    }

    /// Remove a profile by name. Returns whether anything was removed.
    /// If the removed profile was active, picks the first remaining
    /// profile as the new active (or clears `active` when empty).
    pub fn remove_profile(&mut self, name: &str) -> bool {
        let before = self.daemons.len();
        self.daemons.retain(|p| p.name != name);
        let removed = self.daemons.len() != before;
        if removed && self.active.as_deref() == Some(name) {
            self.active = self.daemons.first().map(|p| p.name.clone());
        }
        removed
    }

    /// Switch the active profile. Errors if `name` is unknown.
    pub fn set_active(&mut self, name: &str) -> Result<(), String> {
        if !self.daemons.iter().any(|p| p.name == name) {
            return Err(format!("no profile named '{name}'"));
        }
        self.active = Some(name.to_string());
        Ok(())
    }
}

pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("hone").join("desktop.json"))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn migrate_legacy_sidecar_creates_named_profile() {
        let mut cfg = DesktopConfig {
            daemon: Some(DaemonMode::Sidecar),
            daemons: vec![],
            active: None,
        };
        cfg.migrate_legacy();
        assert!(cfg.daemon.is_none(), "legacy field cleared after migration");
        assert_eq!(cfg.daemons.len(), 1);
        assert_eq!(cfg.daemons[0].name, LEGACY_SIDECAR_NAME);
        assert_eq!(cfg.active.as_deref(), Some(LEGACY_SIDECAR_NAME));
    }

    #[test]
    fn migrate_legacy_external_creates_external_profile() {
        let mut cfg = DesktopConfig {
            daemon: Some(DaemonMode::External {
                url: "http://10.0.0.5:3117".into(),
                token: Some("t".into()),
            }),
            daemons: vec![],
            active: None,
        };
        cfg.migrate_legacy();
        assert_eq!(cfg.daemons.len(), 1);
        assert_eq!(cfg.daemons[0].name, "external");
        assert_eq!(cfg.active.as_deref(), Some("external"));
    }

    #[test]
    fn migrate_legacy_is_idempotent() {
        let mut cfg = DesktopConfig {
            daemon: Some(DaemonMode::Sidecar),
            daemons: vec![],
            active: None,
        };
        cfg.migrate_legacy();
        cfg.migrate_legacy();
        cfg.migrate_legacy();
        assert_eq!(cfg.daemons.len(), 1);
    }

    #[test]
    fn migrate_does_not_clobber_existing_profile_with_same_name() {
        let mut cfg = DesktopConfig {
            daemon: Some(DaemonMode::Sidecar),
            daemons: vec![DaemonProfile {
                name: LEGACY_SIDECAR_NAME.into(),
                mode: DaemonMode::External {
                    url: "http://existing".into(),
                    token: None,
                },
            }],
            active: Some(LEGACY_SIDECAR_NAME.into()),
        };
        cfg.migrate_legacy();
        assert_eq!(cfg.daemons.len(), 1, "no duplicate added");
        // Original entry is preserved (External, not Sidecar).
        assert!(matches!(
            cfg.daemons[0].mode,
            DaemonMode::External { .. }
        ));
    }

    #[test]
    fn add_profile_rejects_empty_name() {
        let mut cfg = DesktopConfig::default();
        let err = cfg
            .add_profile(DaemonProfile {
                name: String::new(),
                mode: DaemonMode::Sidecar,
            })
            .unwrap_err();
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn add_profile_rejects_duplicate_name() {
        let mut cfg = DesktopConfig::default();
        cfg.add_profile(DaemonProfile {
            name: "alpha".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        let err = cfg
            .add_profile(DaemonProfile {
                name: "alpha".into(),
                mode: DaemonMode::Sidecar,
            })
            .unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn add_profile_sets_active_when_none() {
        let mut cfg = DesktopConfig::default();
        cfg.add_profile(DaemonProfile {
            name: "first".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        assert_eq!(cfg.active.as_deref(), Some("first"));

        // Adding a second does not change active.
        cfg.add_profile(DaemonProfile {
            name: "second".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        assert_eq!(cfg.active.as_deref(), Some("first"));
    }

    #[test]
    fn remove_profile_advances_active_when_removing_active() {
        let mut cfg = DesktopConfig::default();
        cfg.add_profile(DaemonProfile {
            name: "a".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        cfg.add_profile(DaemonProfile {
            name: "b".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        assert!(cfg.remove_profile("a"));
        assert_eq!(cfg.active.as_deref(), Some("b"));
    }

    #[test]
    fn remove_profile_clears_active_when_last_one_removed() {
        let mut cfg = DesktopConfig::default();
        cfg.add_profile(DaemonProfile {
            name: "only".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        assert!(cfg.remove_profile("only"));
        assert!(cfg.active.is_none());
        assert!(cfg.daemons.is_empty());
    }

    #[test]
    fn set_active_rejects_unknown_name() {
        let mut cfg = DesktopConfig::default();
        let err = cfg.set_active("nope").unwrap_err();
        assert!(err.contains("no profile named"));
    }

    #[test]
    fn daemon_profile_serialises_as_internally_tagged_kind() {
        let sidecar = DaemonProfile {
            name: "p1".into(),
            mode: DaemonMode::Sidecar,
        };
        let json = serde_json::to_value(&sidecar).expect("ser");
        assert_eq!(json["name"], "p1");
        assert_eq!(json["kind"], "sidecar");
        assert!(json.get("url").is_none(), "sidecar must not carry url");

        let external = DaemonProfile {
            name: "team".into(),
            mode: DaemonMode::External {
                url: "http://10.0.0.5".into(),
                token: Some("t".into()),
            },
        };
        let json = serde_json::to_value(&external).expect("ser");
        assert_eq!(json["kind"], "external");
        assert_eq!(json["url"], "http://10.0.0.5");
        assert_eq!(json["token"], "t");
    }

    #[test]
    fn active_profile_falls_back_to_first_when_pointer_stale() {
        let mut cfg = DesktopConfig::default();
        cfg.add_profile(DaemonProfile {
            name: "a".into(),
            mode: DaemonMode::Sidecar,
        })
        .unwrap();
        cfg.active = Some("does-not-exist".into());
        let active = cfg.active_profile().expect("falls back");
        assert_eq!(active.name, "a");
    }
}
