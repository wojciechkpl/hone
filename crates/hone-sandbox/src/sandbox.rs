use std::process::Command;

use crate::{SandboxError, SandboxPolicy};

/// Opaque handle returned by [`Sandbox::apply`].
///
/// Holding this handle keeps the sandbox configuration live for the lifetime
/// of the value.  Dropping it is a no-op for most backends, but future
/// backends (e.g. kernel namespaces) may use drop to clean up resources.
#[derive(Debug)]
pub struct SandboxHandle {
    _private: (),
}

impl SandboxHandle {
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }
}

/// Which kernel-level mechanism backs a [`Sandbox`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPlatform {
    /// Apple Seatbelt (`sandbox-exec`) — macOS only.
    Seatbelt,
    /// Linux Landlock LSM — Linux ≥ 5.13 only.
    Landlock,
    /// Container-based isolation using `setrlimit` and similar primitives.
    Container,
    /// No-op sandbox used in tests and on unsupported platforms.
    Null,
}

/// Kernel-level sandboxing interface.
///
/// All methods are synchronous and may be called from any thread.  The trait
/// is object-safe so implementations can be stored as `Box<dyn Sandbox>`.
///
/// # Fail-closed semantics
///
/// Every method returns a `Result`.  Callers **must not** proceed if an `Err`
/// is returned — doing so would violate the fail-closed contract.
pub trait Sandbox: Send + Sync + 'static {
    /// Apply the policy to the current process context.
    ///
    /// For Seatbelt this installs a profile via `sandbox_init(3)`.
    /// For Landlock this calls the Landlock syscalls.
    /// For the Null backend this is a no-op that always succeeds.
    fn apply(&self, policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError>;

    /// Prepare `cmd` so that when spawned it runs under `policy`.
    ///
    /// For Seatbelt this prepends `sandbox-exec -p <profile>`.
    /// For Landlock this sets up file-descriptor inheritance.
    /// For the Null backend this is a no-op that always succeeds.
    fn restrict_child(
        &self,
        cmd: &mut Command,
        policy: &SandboxPolicy,
    ) -> Result<(), SandboxError>;

    /// The underlying platform for introspection and diagnostics.
    fn platform(&self) -> SandboxPlatform;
}
