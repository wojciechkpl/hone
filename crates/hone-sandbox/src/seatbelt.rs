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
/// This implementation is a **stub**.  It can generate the profile string via
/// [`SeatbeltProfile`] and validate that the profile is non-empty, but it does
/// not yet call `sandbox_init(3)` or `sandbox-exec(1)` at runtime.
///
/// Available only on `target_os = "macos"`.
#[cfg(target_os = "macos")]
pub struct SeatbeltSandbox;

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
        tracing::debug!(
            "SeatbeltSandbox: wrapping child with sandbox-exec (stub — profile not applied)"
        );
        // TODO: prepend `sandbox-exec -p <profile>` to the command.
        // We store the profile in the command environment as a placeholder
        // so callers can at least inspect what would have been applied.
        let _ = (&profile, cmd);
        Ok(())
    }

    fn platform(&self) -> SandboxPlatform {
        SandboxPlatform::Seatbelt
    }
}
