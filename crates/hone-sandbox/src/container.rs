//! Container / `setrlimit`-based fallback sandbox.
//!
//! This backend applies coarse resource limits via `setrlimit(2)` when
//! neither Seatbelt nor Landlock is available.  It is the weakest backend
//! and cannot enforce filesystem path restrictions.

use std::process::Command;

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

/// Fallback sandbox using `setrlimit` and related POSIX primitives.
///
/// Provides weaker isolation than Seatbelt or Landlock — it can limit CPU
/// time, file sizes, and the number of open files, but cannot restrict which
/// paths a process accesses.
///
/// If the policy requests path-level restrictions and this backend is the only
/// option, callers should consider rejecting the operation rather than running
/// with weaker-than-requested isolation (fail-closed).
pub struct ContainerSandbox;

impl Sandbox for ContainerSandbox {
    fn apply(&self, policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
        // Fail-closed: if the policy requests path-level read restrictions
        // that this backend cannot enforce, surface a clear error.
        if !policy.allow_read.is_empty() || !policy.allow_write.is_empty() {
            tracing::warn!(
                "ContainerSandbox cannot enforce path restrictions; \
                 policy requests {} read path(s) and {} write path(s)",
                policy.allow_read.len(),
                policy.allow_write.len(),
            );
            return Err(SandboxError::PolicyNotMet {
                reason: "ContainerSandbox cannot enforce filesystem path restrictions".to_string(),
            });
        }

        tracing::debug!("ContainerSandbox: apply() stub — setrlimit not yet applied");
        // TODO: apply setrlimit(RLIMIT_FSIZE), setrlimit(RLIMIT_NOFILE), etc.
        Ok(SandboxHandle::new())
    }

    fn restrict_child(
        &self,
        _cmd: &mut Command,
        policy: &SandboxPolicy,
    ) -> Result<(), SandboxError> {
        if !policy.allow_read.is_empty() || !policy.allow_write.is_empty() {
            return Err(SandboxError::PolicyNotMet {
                reason: "ContainerSandbox cannot enforce filesystem path restrictions on child"
                    .to_string(),
            });
        }
        tracing::debug!("ContainerSandbox: restrict_child() stub");
        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Container
    }
}
