//! Loading the permission policy for `hone-mcp-server` (P2.3 spec section 6).

use std::path::{Path, PathBuf};

use hone_config::PermissionError;
use hone_tools::policy_gate::{PolicyConfig, resolve_permission_policy, validate_approvals};

/// Why the policy could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum PolicyLoadError {
    /// A policy file is broken or unreadable (text already sanitised).
    #[error("permission policy error: {0}")]
    Policy(#[from] PermissionError),
    /// `HONE_APPROVE` names a rule that does not exist (text already escaped).
    #[error("{0}")]
    Approve(String),
    /// `HONE_APPROVE` is set but no policy files exist.
    #[error("HONE_APPROVE given but no permission policy is loaded")]
    ApproveWithoutPolicy,
}

/// Load the policy for `working_dir`. `approve` is the raw `HONE_APPROVE` value:
/// comma-separated rule ids, trimmed, empty items ignored. Returns `Ok(None)` when
/// there are no policy files and nothing to approve.
pub fn load_policy(
    working_dir: &Path,
    home: Option<PathBuf>,
    approve: Option<&str>,
) -> Result<Option<PolicyConfig>, PolicyLoadError> {
    let ids: Vec<String> = approve
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let policy = resolve_permission_policy(working_dir, home)?;
    let Some(mut policy) = policy else {
        return if ids.is_empty() {
            Ok(None)
        } else {
            Err(PolicyLoadError::ApproveWithoutPolicy)
        };
    };
    policy.pre_approved = validate_approvals(Some(&policy), &ids)
        .map_err(|m| PolicyLoadError::Approve(m.replacen("--approve", "HONE_APPROVE", 1)))?;
    Ok(Some(policy))
}
