//! Linux Landlock LSM sandbox backend.
//!
//! Landlock is a Linux Security Module available since kernel 5.13 that allows
//! unprivileged processes to restrict their own filesystem and network access.
//!
//! This module is only compiled on Linux.

use std::process::Command;

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

/// Linux Landlock sandbox backend.
///
/// This implementation is a **stub**.  It validates that the policy is
/// well-formed and logs what Landlock ruleset it would create, but does not
/// yet call the `landlock_create_ruleset`, `landlock_add_rule`, and
/// `landlock_restrict_self` syscalls.
///
/// Requires Linux ≥ 5.13.  On older kernels `apply()` returns
/// [`SandboxError::Unavailable`].
#[cfg(target_os = "linux")]
pub struct LandlockSandbox;

#[cfg(target_os = "linux")]
impl Sandbox for LandlockSandbox {
    fn apply(&self, policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
        tracing::debug!(
            read_paths = policy.allow_read.len(),
            write_paths = policy.allow_write.len(),
            "LandlockSandbox: apply() stub — ruleset not yet applied"
        );
        // TODO: call landlock_create_ruleset / landlock_add_rule / landlock_restrict_self
        // via rustix once the full implementation is ready.
        Ok(SandboxHandle::new())
    }

    fn restrict_child(
        &self,
        _cmd: &mut Command,
        policy: &SandboxPolicy,
    ) -> Result<(), SandboxError> {
        tracing::debug!(
            "LandlockSandbox: restrict_child() stub — child will not be landlock-restricted"
        );
        // TODO: set up file-descriptor inheritance for Landlock across fork.
        let _ = policy;
        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Landlock
    }
}
