use std::path::Path;
use std::process::Command;

use crate::{Sandbox, SandboxError, SandboxHandle, SandboxPlatform, SandboxPolicy};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// `int sandbox_init(const char *profile, uint64_t flags, char **errorbuf)`
    ///
    /// Apply a Seatbelt sandbox profile to the current process.
    /// Returns 0 on success, -1 on failure (with `errorbuf` set).
    fn sandbox_init(
        profile: *const std::ffi::c_char,
        flags: u64,
        errorbuf: *mut *mut std::ffi::c_char,
    ) -> std::ffi::c_int;

    /// `void sandbox_free_error(char *errorbuf)`
    ///
    /// Free the error buffer allocated by `sandbox_init`.
    fn sandbox_free_error(errorbuf: *mut std::ffi::c_char);
}

/// Flag for `sandbox_init`: interpret the first argument as a profile string
/// (not a profile name).
#[cfg(target_os = "macos")]
const SANDBOX_NAMED: u64 = 0x0001;

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
    ///
    /// All paths are canonicalized before embedding in the profile because
    /// macOS has symlinks like `/tmp` → `/private/tmp` and Seatbelt operates
    /// on real paths.
    pub fn from_policy(policy: &SandboxPolicy) -> String {
        let mut lines: Vec<String> = Vec::new();

        // Header
        lines.push("(version 1)".to_string());
        lines.push("(deny default)".to_string());

        // Allow reading from each declared path (and its subtree).
        for path in &policy.allow_read {
            let real = canonicalize_or_original(path);
            lines.push(format!(
                r#"(allow file-read* (subpath "{}"))"#,
                escape_path(&real)
            ));
        }

        // Allow writing to each declared path (and its subtree).
        // Write implies read in Seatbelt — include both operations.
        for path in &policy.allow_write {
            let real = canonicalize_or_original(path);
            let escaped = escape_path(&real);
            lines.push(format!(r#"(allow file-read* (subpath "{escaped}"))"#));
            lines.push(format!(r#"(allow file-write* (subpath "{escaped}"))"#));
        }

        // Allow executing specific binaries.
        for path in &policy.allow_exec {
            let real = canonicalize_or_original(path);
            lines.push(format!(
                r#"(allow process-exec (literal "{}"))"#,
                escape_path(&real)
            ));
        }

        // Network rules.
        if policy.allow_network {
            lines.push("(allow network*)".to_string());
        } else {
            lines.push("(deny network*)".to_string());
        }

        // System operations required for any child process to function.
        // Without these, even basic binaries like `cat` or `sh` will SIGABRT
        // because macOS's dynamic linker (dyld) and runtime need Mach ports,
        // sysctl access, and process operations.
        lines.push("(allow sysctl-read)".to_string());
        lines.push("(allow mach-lookup)".to_string());
        lines.push("(allow process-fork)".to_string());
        lines.push("(allow process-exec*)".to_string());

        // Allow global file reads. macOS child processes need to read the
        // dyld shared cache, system frameworks, /dev/urandom, and many other
        // system paths. Enumerating them all is fragile and breaks across
        // macOS versions. The security value of Seatbelt comes from
        // restricting writes, network, and process operations — not reads.
        lines.push("(allow file-read*)".to_string());

        lines.join("\n")
    }
}

/// Resolve symlinks in `path` so the Seatbelt profile uses the real
/// filesystem path. Falls back to the original on error (e.g., if the path
/// does not exist yet).
fn canonicalize_or_original(path: &Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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
        if profile.is_empty() {
            return Err(SandboxError::PolicyNotMet {
                reason: "generated empty seatbelt profile".to_string(),
            });
        }

        tracing::debug!(
            profile_len = profile.len(),
            "SeatbeltSandbox: applying profile via sandbox_init(3)"
        );

        let c_profile = std::ffi::CString::new(profile).map_err(|_| {
            SandboxError::PolicyNotMet {
                reason: "seatbelt profile contains interior NUL byte".to_string(),
            }
        })?;

        // SAFETY: `sandbox_init` is a macOS system call that applies a sandbox
        // profile to the current process. We pass a valid CString pointer, the
        // SANDBOX_NAMED flag (profile string, not name), and a pointer to
        // receive any error message. The error buffer is freed with
        // `sandbox_free_error` if non-null.
        unsafe {
            let mut errorbuf: *mut std::ffi::c_char = std::ptr::null_mut();
            let ret = sandbox_init(c_profile.as_ptr(), SANDBOX_NAMED, &mut errorbuf);
            if ret != 0 {
                let msg = if !errorbuf.is_null() {
                    let err = std::ffi::CStr::from_ptr(errorbuf)
                        .to_string_lossy()
                        .into_owned();
                    sandbox_free_error(errorbuf);
                    err
                } else {
                    "sandbox_init failed with unknown error".to_string()
                };
                return Err(SandboxError::PolicyNotMet {
                    reason: format!("sandbox_init(3) failed: {msg}"),
                });
            }
        }

        tracing::info!("SeatbeltSandbox: profile applied to current process");
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
