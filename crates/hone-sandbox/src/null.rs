use std::process::Command;

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

/// A no-op sandbox that always succeeds.
///
/// `NullSandbox` is the primary test double for code that depends on the
/// [`Sandbox`] trait.  It accepts any policy and never applies any real
/// restrictions, making it suitable for unit tests that do not want to test
/// OS-level enforcement.
///
/// It is also selected by [`crate::SandboxBuilder::detect`] on platforms
/// where no stronger backend is available, ensuring that the rest of the
/// system can always obtain a `Box<dyn Sandbox>` without failing.
///
/// # Example
///
/// ```rust
/// use hone_sandbox::{NullSandbox, Sandbox, SandboxMode, SandboxPolicyBuilder};
///
/// let policy = SandboxPolicyBuilder::new(SandboxMode::Manual).build().unwrap();
/// let handle = NullSandbox.apply(&policy).unwrap();
/// ```
pub struct NullSandbox;

impl Sandbox for NullSandbox {
    fn apply(&self, _policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
        tracing::debug!("NullSandbox: apply() — no restrictions applied");
        Ok(SandboxHandle::new())
    }

    fn restrict_child(
        &self,
        _cmd: &mut Command,
        _policy: &SandboxPolicy,
    ) -> Result<(), SandboxError> {
        tracing::debug!("NullSandbox: restrict_child() — no restrictions applied");
        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Null
    }
}
