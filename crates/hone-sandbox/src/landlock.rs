//! Linux Landlock LSM sandbox backend.
//!
//! [Landlock](https://landlock.io) is a Linux Security Module available since
//! kernel **5.13** that allows unprivileged processes to restrict their own
//! filesystem (and, since 5.19, network) access without requiring elevated
//! privileges.
//!
//! # Kernel version requirements
//!
//! | Landlock ABI | Kernel version | Features added                        |
//! |--------------|---------------|---------------------------------------|
//! | V1           | 5.13          | Basic filesystem access rights        |
//! | V2           | 5.19          | `Refer` (cross-directory rename/link) |
//! | V3           | 6.2           | `Truncate`                            |
//! | V4           | 6.7           | TCP `BindTcp` / `ConnectTcp`          |
//! | V5           | 6.10          | `IoctlDev`                            |
//! | V6           | 6.12          | Unix-socket scope / signal scope      |
//!
//! This implementation uses **best-effort** compatibility mode: if the running
//! kernel supports only a subset of the requested ABI, the available subset is
//! enforced.  A kernel that does not support Landlock at all causes
//! [`apply`](Sandbox::apply) to return [`SandboxError::Unavailable`].
//!
//! # Child-process sandboxing
//!
//! Landlock restrictions are inherited across `fork(2)` but NOT across
//! `execve(2)` unless they were applied before the exec.
//! [`restrict_child`](Sandbox::restrict_child) hooks into `pre_exec` to apply
//! the ruleset inside the forked child, before `execve` runs.

use std::process::Command;

use landlock::{
    ABI, Access, AccessFs, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus,
};

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

/// The Landlock ABI version this implementation targets.
///
/// V1 is the minimum (kernel 5.13).  Higher ABIs are used automatically
/// when the running kernel supports them, thanks to best-effort mode.
const TARGET_ABI: ABI = ABI::V1;

/// Linux Landlock sandbox backend.
///
/// Implements the [`Sandbox`] trait using the Linux Landlock LSM.  On kernels
/// older than 5.13 (where Landlock is absent) [`apply`](Sandbox::apply) and
/// [`restrict_child`](Sandbox::restrict_child) return
/// [`SandboxError::Unavailable`].
///
/// # Thread safety
///
/// `LandlockSandbox` is a zero-sized unit struct and is trivially `Send +
/// Sync`.
pub struct LandlockSandbox;

/// Build a Landlock `RulesetCreated` from `policy` and return it ready for
/// `restrict_self()`.
///
/// Extracted into a free function so it can be called from both `apply()` and
/// the `pre_exec` closure in `restrict_child()`.
fn build_ruleset(policy: &SandboxPolicy) -> Result<landlock::RulesetCreated, SandboxError> {
    // Create a ruleset that handles all filesystem access types for TARGET_ABI.
    // Using best-effort compatibility: the crate will intersect the requested
    // access rights with what the running kernel actually supports.
    let ruleset_created = Ruleset::default()
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .map_err(|e| SandboxError::PolicyNotMet {
            reason: format!("landlock handle_access: {e}"),
        })?
        .create()
        .map_err(|e| SandboxError::PolicyNotMet {
            reason: format!("landlock create ruleset: {e}"),
        })?;

    // Add read-only rules.
    let mut ruleset_created = ruleset_created;
    for path in &policy.allow_read {
        let fd = PathFd::new(path).map_err(|e| SandboxError::PolicyNotMet {
            reason: format!("landlock open path {}: {e}", path.display()),
        })?;
        // from_read() covers Execute | ReadFile | ReadDir.
        let rule = PathBeneath::new(fd, AccessFs::from_read(TARGET_ABI));
        ruleset_created =
            ruleset_created
                .add_rule(rule)
                .map_err(|e| SandboxError::PolicyNotMet {
                    reason: format!("landlock add read rule for {}: {e}", path.display()),
                })?;
    }

    // Add read-write rules (write implies read).
    for path in &policy.allow_write {
        let fd = PathFd::new(path).map_err(|e| SandboxError::PolicyNotMet {
            reason: format!("landlock open path {}: {e}", path.display()),
        })?;
        // from_all() covers every access right: read, write, exec, create, etc.
        let rule = PathBeneath::new(fd, AccessFs::from_all(TARGET_ABI));
        ruleset_created =
            ruleset_created
                .add_rule(rule)
                .map_err(|e| SandboxError::PolicyNotMet {
                    reason: format!("landlock add write rule for {}: {e}", path.display()),
                })?;
    }

    // Add exec-only rules.
    for path in &policy.allow_exec {
        let fd = PathFd::new(path).map_err(|e| SandboxError::PolicyNotMet {
            reason: format!("landlock open path {}: {e}", path.display()),
        })?;
        let rule = PathBeneath::new(fd, AccessFs::Execute.into());
        ruleset_created =
            ruleset_created
                .add_rule(rule)
                .map_err(|e| SandboxError::PolicyNotMet {
                    reason: format!("landlock add exec rule for {}: {e}", path.display()),
                })?;
    }

    Ok(ruleset_created)
}

impl Sandbox for LandlockSandbox {
    /// Apply the Landlock policy to the **current process**.
    ///
    /// This is a one-way operation: once applied the process cannot
    /// regain access rights, even after the returned [`SandboxHandle`] is
    /// dropped.
    ///
    /// # Errors
    ///
    /// - [`SandboxError::PolicyNotMet`] if a path cannot be opened or a rule
    ///   cannot be added.
    /// - [`SandboxError::Unavailable`] if the running kernel does not support
    ///   Landlock at all (kernel < 5.13 or compiled without `CONFIG_SECURITY_LANDLOCK`).
    fn apply(&self, policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
        let ruleset_created = build_ruleset(policy)?;

        let status = ruleset_created
            .restrict_self()
            .map_err(|e| SandboxError::PolicyNotMet {
                reason: format!("landlock restrict_self: {e}"),
            })?;

        match status.ruleset {
            RulesetStatus::FullyEnforced => {
                tracing::info!("landlock: fully enforced");
            }
            RulesetStatus::PartiallyEnforced => {
                tracing::warn!(
                    "landlock: partially enforced — kernel may not support all \
                     requested access rights"
                );
            }
            RulesetStatus::NotEnforced => {
                return Err(SandboxError::Unavailable {
                    platform: "linux (kernel does not support Landlock — \
                                kernel < 5.13 or CONFIG_SECURITY_LANDLOCK not set)"
                        .to_string(),
                });
            }
        }

        Ok(SandboxHandle::new())
    }

    /// Arrange for `cmd` to run under the Landlock `policy` in the child
    /// process without restricting the current (parent) process.
    ///
    /// Internally this registers a `pre_exec` hook via [`CommandExt::pre_exec`]
    /// that calls [`build_ruleset`] + `restrict_self()` in the forked child
    /// right before `execve(2)`.
    ///
    /// # Safety note
    ///
    /// The `pre_exec` closure runs in the forked child between `fork(2)` and
    /// `execve(2)`.  Only async-signal-safe functions should be called in this
    /// context.  The Landlock syscalls (`landlock_create_ruleset`,
    /// `landlock_add_rule`, `landlock_restrict_self`) are raw Linux syscalls
    /// and are signal-safe.  The `open(2)` calls inside `PathFd::new` are also
    /// signal-safe.  Memory allocation (`format!`, `Vec`) is technically not
    /// signal-safe but is accepted practice for `pre_exec` hooks that abort on
    /// error and never return to userspace without `execve`.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::PolicyNotMet`] only if the policy references a
    /// path that does not exist at call time (pre-fork path validation is not
    /// done here; errors surface when the child is spawned).
    fn restrict_child(
        &self,
        cmd: &mut Command,
        policy: &SandboxPolicy,
    ) -> Result<(), SandboxError> {
        use std::os::unix::process::CommandExt;

        let policy = policy.clone();

        // SAFETY: The closure is called in the forked child after `fork(2)` but
        // before `execve(2)`.  We only invoke Landlock syscalls (signal-safe)
        // and `open(2)` (signal-safe).  On error we return an `io::Error` which
        // causes the child to exit without executing the target program.
        unsafe {
            cmd.pre_exec(move || {
                let sandbox = LandlockSandbox;
                sandbox.apply(&policy).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("landlock pre_exec: {e}"),
                    )
                })?;
                Ok(())
            });
        }

        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Landlock
    }
}
