pub mod validation;

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use hone_sandbox::{
    SandboxError, SandboxMode, SandboxPlatform, SandboxPolicy, SandboxPolicyBuilder,
};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

/// System directories made readable to shell commands (those that exist).
///
/// macOS: only the Homebrew prefix subdirs, never the whole of `/opt` or
/// `/usr/local` (which hold `var`, `etc`, caches, credentials, ...).
#[cfg(target_os = "macos")]
pub const SYSTEM_READ_ROOTS: &[&str] = &[
    "/bin",
    "/usr/bin",
    "/usr/lib",
    "/usr/share",
    "/etc",
    "/usr/local/bin",
    "/usr/local/lib",
    "/usr/local/opt",
    "/usr/local/Cellar",
    "/usr/local/share",
    "/usr/local/include",
    "/opt/homebrew/bin",
    "/opt/homebrew/lib",
    "/opt/homebrew/opt",
    "/opt/homebrew/Cellar",
    "/opt/homebrew/share",
    "/opt/homebrew/include",
    "/opt/homebrew/etc/gitconfig",
    "/usr/local/etc/gitconfig",
];

/// System directories made readable to shell commands (those that exist).
#[cfg(not(target_os = "macos"))]
pub const SYSTEM_READ_ROOTS: &[&str] = &[
    "/bin",
    "/usr/bin",
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/usr/share",
    "/etc",
    "/opt",
    "/usr/local",
];

/// Paths under `$HOME` made readable when they exist (dirs or files).
pub const HOME_READ_DIRS: &[&str] = &[
    ".cache",
    ".cargo",
    ".rustup",
    ".gitconfig",
    ".config/git/config",
    ".config/git/ignore",
    ".config/git/attributes",
];

/// Device nodes made writable when they exist.
pub const WRITE_DEVICES: &[&str] = &["/dev/null"];

/// Env var holding extra read-only roots for the shell sandbox, separated by
/// `:`. Relative and nonexistent entries are ignored, as are `/`, `$HOME` or
/// its ancestors, and anything under the working dir or `/tmp` (including
/// symlinks resolving there).
pub const EXTRA_READ_ENV: &str = "HONE_SANDBOX_EXTRA_READ";

/// Env var that must be exactly `1` to enforce the Landlock sandbox on Linux
/// (opt-in for now). Other platforms always enforce.
pub const SANDBOX_ENFORCE_ENV: &str = "HONE_SANDBOX_ENFORCE";

/// Whether the shell tool should install the sandbox on `platform`, given the
/// value of [`SANDBOX_ENFORCE_ENV`].
pub fn sandbox_enforced(platform: SandboxPlatform, enforce_env: Option<&str>) -> bool {
    match platform {
        SandboxPlatform::Landlock => enforce_env == Some("1"),
        _ => true,
    }
}

/// Parse a `:`-separated list of extra read roots. Entries are trimmed; empty
/// and relative entries are dropped.
pub fn parse_extra_read(raw: &str) -> Vec<PathBuf> {
    raw.split(':')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
}

/// The path as written plus its canonical form (when resolvable).
fn path_forms(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    if let Ok(c) = p.canonicalize()
        && c != p
    {
        v.push(c);
    }
    v
}

/// Whether an extra read root is too broad or overlaps writable/home areas.
fn extra_rejected(extra: &Path, home: &[PathBuf], writable: &[PathBuf]) -> bool {
    path_forms(extra).iter().any(|f| {
        f.parent().is_none()
            || home.iter().any(|h| h.starts_with(f))
            || writable.iter().any(|w| f.starts_with(w))
    })
}

/// Build the shell tool's sandbox policy. Reads no environment variables.
///
/// Only existing paths are added (Landlock fails on missing ones), except
/// `working_dir`, which is always writable. Writes: `working_dir`, `/tmp`,
/// [`WRITE_DEVICES`]. Reads: system roots, `$HOME` entries (skipped when under
/// `working_dir`), then filtered `extra_read` entries; duplicates are removed.
pub fn shell_sandbox_policy(
    working_dir: &Path,
    home: Option<&Path>,
    extra_read: &[PathBuf],
) -> Result<SandboxPolicy, SandboxError> {
    let mut builder =
        SandboxPolicyBuilder::new(SandboxMode::Autonomous).allow_write(working_dir.to_path_buf());
    let tmp = PathBuf::from("/tmp");
    if tmp.exists() {
        builder = builder.allow_write(tmp);
    }
    for dev in WRITE_DEVICES {
        let dev = PathBuf::from(dev);
        if dev.exists() {
            builder = builder.allow_write(dev);
        }
    }

    // Literal and canonical forms of every writable root / the home dir.
    let writable: Vec<PathBuf> = path_forms(working_dir)
        .into_iter()
        .chain(path_forms(Path::new("/tmp")))
        .collect();
    let home_forms: Vec<PathBuf> = home.map(path_forms).unwrap_or_default();

    let home_dirs = home.into_iter().flat_map(|h| {
        HOME_READ_DIRS
            .iter()
            .map(move |d| h.join(d))
            .filter(|p| !p.starts_with(working_dir))
    });
    let extras = extra_read
        .iter()
        .filter(|p| p.is_absolute() && !extra_rejected(p, &home_forms, &writable))
        .cloned();
    let candidates = SYSTEM_READ_ROOTS
        .iter()
        .map(PathBuf::from)
        .chain(home_dirs)
        .chain(extras);

    let mut seen: Vec<PathBuf> = Vec::new();
    for p in candidates {
        if p.exists() && !seen.contains(&p) {
            seen.push(p.clone());
            builder = builder.allow_read(p);
        }
    }
    builder.build()
}

/// Executes a shell command inside the sandbox and captures stdout/stderr.
pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its stdout, stderr, and exit code.\n\n\
         WHEN TO USE: To run tests (cargo test, pytest, npm test). To check build status. \
         To run linters or formatters. To execute git commands not covered by git_* tools.\n\
         WHEN NOT TO USE: To read files — use read_file (faster, line-numbered). \
         To search files — use grep (faster, respects gitignore). \
         To list files — use glob (structured output).\n\
         COST: Spawns a subprocess. Use specific tools when available — they are faster and safer.\n\
         SAFETY: Destructive commands (rm -rf, sudo) and network commands (curl, wget) are blocked. \
         Compound operators (;, &&, ||, |) are treated as destructive."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds. Defaults to 30 000.",
                    "minimum": 1
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let command = input
            .get_str("command")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'command'".to_string(),
            })?
            .to_string();

        // Validate the command before executing.
        match validation::validate_command(&command) {
            validation::ValidationResult::Allow => {}
            validation::ValidationResult::Block(reason) => {
                return Err(ToolError::InvalidInput {
                    reason: format!("command blocked: {reason}"),
                });
            }
            validation::ValidationResult::RequireApproval(reason) => {
                return Err(ToolError::InvalidInput {
                    reason: format!("command blocked (requires approval): {reason}"),
                });
            }
        }

        let timeout_ms = input
            .get_u64("timeout_ms")
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // Build a std::process::Command first so we can pass it through
        // restrict_child (which takes &mut std::process::Command).  The
        // sandbox may rewrite the command entirely (e.g. prepend sandbox-exec),
        // so we capture the final program and args afterwards and reconstruct a
        // tokio::process::Command from them.
        let mut std_cmd = std::process::Command::new("sh");
        std_cmd.arg("-c").arg(&command);

        // Build a concrete policy anchored to the working directory.
        //
        // Minimum surfaces (see `shell_sandbox_policy`):
        //   - working_dir: read + write (the agent's project root)
        //   - /tmp: writable temp space, when present
        //   - SYSTEM_READ_ROOTS (/bin, /usr/bin, /usr/lib, /usr/share, /usr/local,
        //     /opt, /etc): readable so interpreters, toolchains (conda under /opt,
        //     python under /usr/local, Homebrew) and config files work
        //   - $HOME/{.cache,.cargo,.rustup}: readable toolchain caches, only when
        //     they exist and are not under working_dir
        //   - HONE_SANDBOX_EXTRA_READ: operator-supplied extra read roots
        //
        // SandboxMode::Autonomous requires at least one readable path and is
        // the most restrictive mode available, so the builder rejects a
        // misconfigured policy at build time.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let extra = parse_extra_read(
            &std::env::var_os(EXTRA_READ_ENV)
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let policy =
            shell_sandbox_policy(&ctx.working_dir, home.as_deref(), &extra).map_err(|e| {
                ToolError::SandboxDenied {
                    tool: "shell".to_string(),
                    action: format!("policy build failed: {e}"),
                }
            })?;

        let platform = ctx.sandbox.platform();
        let enforce_env = std::env::var(SANDBOX_ENFORCE_ENV).ok();
        if sandbox_enforced(platform, enforce_env.as_deref()) {
            ctx.sandbox
                .restrict_child(&mut std_cmd, &policy)
                .map_err(|e| ToolError::SandboxDenied {
                    tool: "shell".to_string(),
                    action: e.to_string(),
                })?;
        } else {
            static WARN_ONCE: Once = Once::new();
            WARN_ONCE.call_once(|| {
                tracing::warn!(
                    "Linux shell sandboxing (Landlock) is opt-in: set \
                     {SANDBOX_ENFORCE_ENV}=1 to enforce it; shell commands currently run \
                     unsandboxed"
                );
            });
        }

        // Convert directly so `pre_exec` hooks and any program/arg rewrite done
        // by the sandbox (e.g. Seatbelt's sandbox-exec) are preserved.
        let mut cmd = Command::from(std_cmd);
        cmd.current_dir(&ctx.working_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let duration = Duration::from_millis(timeout_ms);
        let result = timeout(duration, cmd.output()).await;

        match result {
            Err(_elapsed) => Err(ToolError::Timeout {
                tool: "shell".to_string(),
                duration_ms: timeout_ms,
            }),
            Ok(Err(io_err)) => Err(ToolError::IoError(io_err)),
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let exit_code = output.status.code().unwrap_or(-1);

                Ok(ToolOutput::new(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code
                })))
            }
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
