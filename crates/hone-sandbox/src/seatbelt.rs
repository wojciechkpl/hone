use std::path::Path;
use std::process::Command;

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

/// Generates Apple Seatbelt (TinyScheme-based) profile strings from a
/// [`SandboxPolicy`].
///
/// Seatbelt profiles use a Scheme-like DSL interpreted by `libsandbox` /
/// `sandbox-exec`.  The format starts with `(version 1)`, followed by an
/// ordered list of allow/deny rules.  Rules are evaluated top-to-bottom and
/// the last matching rule wins — so the profile ends with `(deny default)` to
/// provide a catch-all deny after any explicit allows.
pub struct SeatbeltProfile;

impl SeatbeltProfile {
    /// Render a complete Seatbelt profile string for `policy`.
    pub fn from_policy(policy: &SandboxPolicy) -> String {
        let mut lines: Vec<String> = Vec::new();

        // Header
        lines.push("(version 1)".to_string());
        lines.push("(deny default)".to_string());

        // Allow reading from each declared path (and its subtree).
        for path in &policy.allow_read {
            lines.push(format!(
                r#"(allow file-read* (subpath "{}"))"#,
                escape_path(path)
            ));
        }

        // Allow writing to each declared path (and its subtree).
        // Write implies read in Seatbelt — include both operations.
        for path in &policy.allow_write {
            let escaped = escape_path(path);
            lines.push(format!(r#"(allow file-read* (subpath "{escaped}"))"#));
            lines.push(format!(r#"(allow file-write* (subpath "{escaped}"))"#));
        }

        // Allow executing specific binaries.
        for path in &policy.allow_exec {
            lines.push(format!(
                r#"(allow process-exec (literal "{}"))"#,
                escape_path(path)
            ));
        }

        // Network rules.
        if policy.allow_network {
            lines.push("(allow network*)".to_string());
        } else {
            lines.push("(deny network*)".to_string());
        }

        // System calls that are always safe to permit.
        lines.push("(allow sysctl-read)".to_string());
        lines.push("(allow mach-lookup)".to_string());

        lines.join("\n")
    }
}

/// Escape a path for embedding inside a Seatbelt profile string literal.
///
/// Seatbelt paths are quoted strings in a Scheme-like syntax, so we only need
/// to escape backslashes and double-quotes.
fn escape_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// macOS Seatbelt sandbox backend.
///
/// Wraps child processes with `sandbox-exec -p <profile>` to enforce the
/// policy at the kernel level.
///
/// Available only on `target_os = "macos"`.
#[cfg(target_os = "macos")]
pub struct SeatbeltSandbox;

#[cfg(target_os = "macos")]
impl SeatbeltSandbox {
    /// Build a `Command` that runs `program` under `sandbox-exec` with the
    /// Seatbelt profile derived from `policy`.
    ///
    /// This is the preferred way to launch a sandboxed subprocess when you
    /// control the initial `Command` construction.  [`restrict_child`] handles
    /// the case where the caller has already partially constructed a `Command`.
    ///
    /// [`restrict_child`]: Sandbox::restrict_child
    pub fn sandboxed_command(
        program: &str,
        policy: &SandboxPolicy,
    ) -> Result<Command, SandboxError> {
        let profile = SeatbeltProfile::from_policy(policy);
        if profile.is_empty() {
            return Err(SandboxError::PolicyNotMet {
                reason: "generated empty seatbelt profile".to_string(),
            });
        }
        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-p").arg(profile).arg(program);
        Ok(cmd)
    }
}

#[cfg(target_os = "macos")]
impl Sandbox for SeatbeltSandbox {
    fn apply(&self, policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
        let profile = SeatbeltProfile::from_policy(policy);
        tracing::debug!(
            profile_len = profile.len(),
            "SeatbeltSandbox: generated profile (apply stub)"
        );
        // TODO: call sandbox_init(3) with the generated profile.
        // For now, just validate the profile is non-empty as a smoke test.
        if profile.is_empty() {
            return Err(SandboxError::PolicyNotMet {
                reason: "generated empty seatbelt profile".to_string(),
            });
        }
        Ok(SandboxHandle::new())
    }

    fn restrict_child(
        &self,
        cmd: &mut Command,
        policy: &SandboxPolicy,
    ) -> Result<(), SandboxError> {
        let profile = SeatbeltProfile::from_policy(policy);
        if profile.is_empty() {
            return Err(SandboxError::PolicyNotMet {
                reason: "generated empty seatbelt profile".to_string(),
            });
        }

        // Capture the original program and arguments from the existing command.
        let program = cmd.get_program().to_os_string();
        let args: Vec<std::ffi::OsString> =
            cmd.get_args().map(|a| a.to_os_string()).collect();

        // Rebuild the command as: sandbox-exec -p <profile> <original_program> <original_args>
        *cmd = Command::new("sandbox-exec");
        cmd.arg("-p").arg(&profile);
        cmd.arg(&program);
        for arg in &args {
            cmd.arg(arg);
        }

        tracing::debug!(
            profile_len = profile.len(),
            "SeatbeltSandbox: wrapped child with sandbox-exec"
        );
        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Seatbelt
    }
}
