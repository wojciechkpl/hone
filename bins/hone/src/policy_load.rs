//! Loading the permission policy for CLI runs (P2.2 spec section 7).

use std::path::{Path, PathBuf};

use hone_config::PermissionError;
use hone_config::policy::{CompiledPolicy, PolicyContext};
use hone_core::PolicyConfig;

const USER_FILE: [&str; 3] = [".config", "hone", "permissions.toml"];
const PROJECT_FILE: [&str; 2] = [".hone", "permissions.toml"];

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Resolve the permission policy for an agent running in `working_dir`.
///
/// Returns `Ok(None)` when neither the user file
/// (`<home>/.config/hone/permissions.toml`) nor the project file
/// (`<workdir>/.hone/permissions.toml`) exists, which is exactly the
/// behaviour without a policy. A relative `home` counts as unknown.
pub(crate) fn resolve_permission_policy(
    working_dir: &Path,
    home: Option<PathBuf>,
) -> Result<Option<PolicyConfig>, PermissionError> {
    let home = home.filter(|h| h.is_absolute());
    // A workdir that can't be canonicalised is only an error once there is a
    // policy file to apply (no files means no policy, as today).
    let (workdir, canon_err) = match std::fs::canonicalize(working_dir) {
        Ok(w) => (w, None),
        Err(e) => (working_dir.to_path_buf(), Some(e)),
    };

    let user = home
        .as_ref()
        .map(|h| USER_FILE.iter().fold(h.clone(), |p, c| p.join(c)))
        .filter(|p| exists(p));
    let project = PROJECT_FILE.iter().fold(workdir.clone(), |p, c| p.join(c));
    if user.is_none() && !exists(&project) {
        return Ok(None);
    }

    if let Some(e) = canon_err {
        return Err(PermissionError::Io(e));
    }

    let context = PolicyContext::for_platform(workdir, home);
    let compiled = CompiledPolicy::load_layers(&context, &[], user.as_deref(), &project)?;
    Ok(Some(PolicyConfig {
        compiled: std::sync::Arc::new(compiled),
        context,
        pre_approved: Vec::new(),
    }))
}

#[cfg(test)]
#[path = "policy_load_tests.rs"]
mod tests;
