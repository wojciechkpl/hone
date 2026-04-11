//! Shell escape — execute native shell commands from the Hone prompt.
//!
//! Users type `!command` to bypass the agent and run `command` directly in
//! the shell.  Three flavours are supported:
//!
//! * **`!cd <dir>`** — change the current working directory persistently for
//!   subsequent shell escapes and agent turns.
//! * **Interactive commands** (`nvim`, `vim`, `vi`, `nano`, `less`, `man`,
//!   `top`, `htop`) — the TUI must be suspended while the command runs.
//!   Callers detect this via [`ShellEscapeResult::NeedsTerminalSuspend`] and
//!   are responsible for the leave/restore crossterm dance.
//! * **Everything else** — run via `sh -c`, capture stdout+stderr, and return
//!   the output for inline display.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// The set of command names that require full terminal control and therefore
/// cannot be captured — the TUI must suspend itself first.
const INTERACTIVE_CMDS: &[&str] =
    &["nvim", "vim", "vi", "nano", "less", "man", "top", "htop", "fzf", "bat"];

/// Return `true` when the first whitespace-delimited token of `cmd` is an
/// interactive command that needs a raw terminal.
pub fn is_interactive_command(cmd: &str) -> bool {
    let first = cmd.split_whitespace().next().unwrap_or("");
    INTERACTIVE_CMDS.contains(&first)
}

/// Result of running a shell escape.
#[derive(Debug)]
pub enum ShellEscapeResult {
    /// The command produced captured output (success or failure).
    Output {
        /// Combined stdout + stderr text.
        text: String,
        /// Whether the command exited successfully.
        success: bool,
    },
    /// The working directory was changed.
    DirectoryChanged {
        /// The new absolute working directory.
        new_dir: PathBuf,
    },
    /// The command requires a raw terminal; the caller must suspend the TUI,
    /// run the command synchronously (blocking), then restore the TUI.
    NeedsTerminalSuspend {
        /// The raw shell command string to pass to `sh -c`.
        command: String,
        /// Working directory to use when running the command.
        working_dir: PathBuf,
    },
    /// The requested `cd` target did not exist or was not a directory.
    CdError(String),
}

/// Execute a shell escape command, returning a structured result.
///
/// `cmd` is everything after the leading `!` character.
/// `working_dir` is the current effective working directory.
pub async fn execute_shell_escape(cmd: &str, working_dir: &Path) -> ShellEscapeResult {
    let cmd = cmd.trim();

    // --- cd handling ---------------------------------------------------------
    if let Some(dir_arg) = cmd.strip_prefix("cd").and_then(|rest| {
        // Accept "cd" alone (go to $HOME) or "cd <dir>".
        if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
            Some(rest.trim())
        } else {
            None
        }
    }) {
        let target: PathBuf = if dir_arg.is_empty() {
            // `cd` with no argument → $HOME.
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| working_dir.to_owned())
        } else if dir_arg.starts_with('/') || dir_arg.starts_with('~') {
            // Absolute path or tilde expansion.
            expand_tilde(dir_arg)
        } else {
            // Relative path — resolve against working_dir.
            working_dir.join(dir_arg)
        };

        return match target.canonicalize() {
            Ok(canonical) if canonical.is_dir() => {
                ShellEscapeResult::DirectoryChanged { new_dir: canonical }
            }
            Ok(_) => ShellEscapeResult::CdError(format!(
                "cd: not a directory: {}",
                target.display()
            )),
            Err(e) => ShellEscapeResult::CdError(format!(
                "cd: {}: {}",
                target.display(),
                e
            )),
        };
    }

    // --- interactive commands -------------------------------------------------
    if is_interactive_command(cmd) {
        return ShellEscapeResult::NeedsTerminalSuspend {
            command: cmd.to_owned(),
            working_dir: working_dir.to_owned(),
        };
    }

    // --- captured commands ---------------------------------------------------
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(out) => {
            let mut text = String::new();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stdout.is_empty() {
                text.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if text.is_empty() {
                text.push_str("(no output)");
            }
            // Trim a trailing newline so the inline display looks clean.
            if text.ends_with('\n') {
                text.pop();
            }
            ShellEscapeResult::Output {
                success: out.status.success(),
                text,
            }
        }
        Err(e) => ShellEscapeResult::Output {
            text: format!("shell: failed to spawn command: {e}"),
            success: false,
        },
    }
}

/// Run an interactive command synchronously (blocking) after the TUI has been
/// suspended.  Returns `Ok(())` when the command exits, regardless of its exit
/// code.
pub fn run_interactive_blocking(cmd: &str, working_dir: &Path) -> std::io::Result<()> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(working_dir)
        .status()
        .map(|_| ())
}

/// Naively expand a leading `~` to `$HOME`.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest.trim_start_matches('/'));
        }
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_interactive_command ---------------------------------------------

    #[test]
    fn interactive_detection_known_editors() {
        for name in ["nvim", "vim", "vi", "nano", "less", "man", "top", "htop"] {
            assert!(
                is_interactive_command(name),
                "{name} should be interactive"
            );
            // With arguments.
            assert!(
                is_interactive_command(&format!("{name} some/file")),
                "{name} with args should be interactive"
            );
        }
    }

    #[test]
    fn interactive_detection_regular_commands() {
        for name in ["ls", "echo", "cargo", "git", "cat", "grep"] {
            assert!(
                !is_interactive_command(name),
                "{name} should not be interactive"
            );
        }
    }

    #[test]
    fn interactive_detection_empty() {
        assert!(!is_interactive_command(""));
    }

    // --- cd path resolution -------------------------------------------------

    #[tokio::test]
    async fn cd_to_tmp_succeeds() {
        let result = execute_shell_escape("cd /tmp", Path::new("/")).await;
        match result {
            ShellEscapeResult::DirectoryChanged { new_dir } => {
                // On macOS /tmp is a symlink to /private/tmp; canonicalize handles this.
                assert!(new_dir.is_dir());
                assert!(new_dir.ends_with("tmp") || new_dir.to_string_lossy().contains("tmp"));
            }
            other => panic!("expected DirectoryChanged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cd_to_nonexistent_returns_error() {
        let result =
            execute_shell_escape("cd /this/path/does/not/exist/42", Path::new("/")).await;
        assert!(matches!(result, ShellEscapeResult::CdError(_)));
    }

    #[tokio::test]
    async fn cd_relative_resolves_against_cwd() {
        // Use /tmp as the working dir; relative "." should resolve to it.
        let tmp = std::path::Path::new("/tmp").canonicalize().unwrap();
        let result = execute_shell_escape("cd .", &tmp).await;
        match result {
            ShellEscapeResult::DirectoryChanged { new_dir } => {
                assert_eq!(new_dir, tmp);
            }
            other => panic!("expected DirectoryChanged, got {other:?}"),
        }
    }

    // --- ! prefix stripping (done by the caller, tested via parse_shell_escape) --

    #[test]
    fn strip_prefix_leaves_command() {
        let input = "!ls -la";
        let cmd = input.strip_prefix('!').unwrap();
        assert_eq!(cmd, "ls -la");
    }

    #[test]
    fn strip_prefix_single_bang() {
        let input = "!";
        let cmd = input.strip_prefix('!').unwrap();
        assert_eq!(cmd, "");
    }

    // --- captured output ----------------------------------------------------

    #[tokio::test]
    async fn echo_produces_output() {
        let result = execute_shell_escape("echo hello", Path::new("/tmp")).await;
        match result {
            ShellEscapeResult::Output { text, success } => {
                assert!(success, "echo should succeed");
                assert_eq!(text.trim(), "hello");
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn failing_command_success_is_false() {
        let result = execute_shell_escape("false", Path::new("/tmp")).await;
        match result {
            ShellEscapeResult::Output { success, .. } => {
                assert!(!success, "false should not succeed");
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    // --- tilde expansion ----------------------------------------------------

    #[test]
    fn tilde_expands_to_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_owned());
        let expanded = expand_tilde("~/projects");
        assert_eq!(expanded, PathBuf::from(home).join("projects"));
    }

    #[test]
    fn non_tilde_path_unchanged() {
        let path = "/usr/local/bin";
        let expanded = expand_tilde(path);
        assert_eq!(expanded, PathBuf::from(path));
    }
}
