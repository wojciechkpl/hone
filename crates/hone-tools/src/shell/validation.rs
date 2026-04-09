//! Command intent classification and staged validation.
//!
//! Before executing a shell command, classify its intent (ReadOnly,
//! Mutation, Destructive, Network, PrivilegeEscalation) and apply
//! validators appropriate to each intent level.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandIntent {
    /// No filesystem or state changes (ls, cat, grep, find, pwd, ps, etc.)
    ReadOnly,
    /// Modifies files in the working directory (mkdir, touch, sed -i, mv)
    Mutation,
    /// Irreversible destructive ops (rm -rf, dd, mkfs, truncate)
    Destructive,
    /// Network operations (curl, wget, ssh, nc, git push)
    Network,
    /// Privilege escalation (sudo, su, doas)
    PrivilegeEscalation,
    /// Unknown/unclassified
    Unknown,
}

impl CommandIntent {
    /// Classify a command line by its first token and key flags.
    pub fn classify(command: &str) -> Self {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Self::Unknown;
        }

        // Split on the first whitespace to get the program name.
        // Also handle `sudo foo ...` and `env FOO=bar prog ...` prefixes.
        let first_token = trimmed.split_whitespace().next().unwrap_or("");

        // Privilege escalation — detected on the leading token
        if matches!(first_token, "sudo" | "su" | "doas" | "pkexec") {
            return Self::PrivilegeEscalation;
        }

        // Destructive patterns — check for specific dangerous invocations
        if is_destructive(trimmed) {
            return Self::Destructive;
        }

        // Network commands
        if matches!(
            first_token,
            "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc" | "netcat" | "ftp"
        ) {
            return Self::Network;
        }

        // Git network subcommands
        if first_token == "git" {
            let rest = trimmed[3..].trim_start();
            if rest.starts_with("push")
                || rest.starts_with("pull")
                || rest.starts_with("fetch")
                || rest.starts_with("clone")
                || rest.starts_with("remote")
            {
                return Self::Network;
            }
        }

        // Read-only commands
        if matches!(
            first_token,
            "ls" | "cat"
                | "head"
                | "tail"
                | "less"
                | "more"
                | "grep"
                | "rg"
                | "find"
                | "fd"
                | "pwd"
                | "ps"
                | "top"
                | "htop"
                | "which"
                | "whoami"
                | "date"
                | "env"
                | "printenv"
                | "file"
                | "stat"
                | "wc"
                | "sort"
                | "uniq"
                | "diff"
                | "md5sum"
                | "sha256sum"
                | "echo"
                | "true"
                | "false"
                | "test"
        ) {
            return Self::ReadOnly;
        }

        // Git read-only subcommands
        if first_token == "git" {
            let rest = trimmed[3..].trim_start();
            if rest.starts_with("status")
                || rest.starts_with("log")
                || rest.starts_with("diff")
                || rest.starts_with("show")
                || rest.starts_with("branch")
                || rest.starts_with("blame")
                || rest.starts_with("config -l")
            {
                return Self::ReadOnly;
            }
        }

        // Cargo read-only
        if first_token == "cargo" {
            let rest = trimmed[5..].trim_start();
            if rest.starts_with("check")
                || rest.starts_with("tree")
                || rest.starts_with("metadata")
                || rest.starts_with("--version")
            {
                return Self::ReadOnly;
            }
        }

        // Mutation commands — catch-all for filesystem writes
        if matches!(
            first_token,
            "mkdir" | "touch" | "cp" | "mv" | "ln" | "chmod" | "chown" | "tar" | "unzip"
                | "zip" | "gzip" | "gunzip"
        ) {
            return Self::Mutation;
        }

        // Default: Unknown (which the caller can treat as Mutation for safety)
        Self::Unknown
    }

    /// Whether this intent requires explicit user approval by default.
    pub fn requires_approval(self) -> bool {
        matches!(
            self,
            Self::Destructive | Self::PrivilegeEscalation | Self::Network
        )
    }
}

/// Check whether a command line contains destructive patterns.
fn is_destructive(command: &str) -> bool {
    let patterns = [
        "rm -rf",
        "rm -fr",
        "rm -Rf",
        "rm --recursive --force",
        "dd if=",
        "dd of=",
        "mkfs",
        "fdisk",
        "parted",
        ": > /",
        "> /dev/",
        "truncate",
        "find / -delete",
        "find . -delete",
        "chmod -R",
        "chown -R",
        "git reset --hard",
        "git clean -fd",
        "git clean -fdx",
        "git push --force",
        "git push -f",
        ":(){ :|:& };:",
    ];

    let lower = command.to_lowercase();
    patterns
        .iter()
        .any(|p| lower.contains(&p.to_lowercase()))
}

/// A validator runs against a classified command and either allows it,
/// blocks it with a reason, or signals that approval is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// Command is allowed to run.
    Allow,
    /// Command is blocked with a reason.
    Block(String),
    /// Command requires user approval before running.
    RequireApproval(String),
}

/// Run the staged validator pipeline on a command.
///
/// Default policy:
/// - ReadOnly: allow
/// - Mutation, Unknown: allow (scoped by working_dir via sandbox)
/// - Destructive: require approval
/// - Network: require approval
/// - PrivilegeEscalation: block
pub fn validate_command(command: &str) -> ValidationResult {
    let intent = CommandIntent::classify(command);
    match intent {
        CommandIntent::ReadOnly => ValidationResult::Allow,
        CommandIntent::Mutation | CommandIntent::Unknown => ValidationResult::Allow,
        CommandIntent::Destructive => ValidationResult::RequireApproval(format!(
            "destructive command detected: {}",
            command
        )),
        CommandIntent::Network => ValidationResult::RequireApproval(format!(
            "network command detected: {}",
            command
        )),
        CommandIntent::PrivilegeEscalation => ValidationResult::Block(
            "privilege escalation commands (sudo/su/doas) are not permitted".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_read_only() {
        assert_eq!(CommandIntent::classify("ls -la"), CommandIntent::ReadOnly);
        assert_eq!(
            CommandIntent::classify("cat src/main.rs"),
            CommandIntent::ReadOnly
        );
        assert_eq!(
            CommandIntent::classify("grep -r foo ."),
            CommandIntent::ReadOnly
        );
        assert_eq!(
            CommandIntent::classify("git status"),
            CommandIntent::ReadOnly
        );
        assert_eq!(
            CommandIntent::classify("cargo check"),
            CommandIntent::ReadOnly
        );
    }

    #[test]
    fn test_classify_mutation() {
        assert_eq!(CommandIntent::classify("mkdir foo"), CommandIntent::Mutation);
        assert_eq!(CommandIntent::classify("cp a b"), CommandIntent::Mutation);
        assert_eq!(CommandIntent::classify("touch x"), CommandIntent::Mutation);
    }

    #[test]
    fn test_classify_destructive() {
        assert_eq!(
            CommandIntent::classify("rm -rf /tmp/foo"),
            CommandIntent::Destructive
        );
        assert_eq!(
            CommandIntent::classify("git reset --hard HEAD~5"),
            CommandIntent::Destructive
        );
        assert_eq!(
            CommandIntent::classify("git push --force origin main"),
            CommandIntent::Destructive
        );
        assert_eq!(
            CommandIntent::classify("dd if=/dev/zero of=/dev/sda"),
            CommandIntent::Destructive
        );
    }

    #[test]
    fn test_classify_network() {
        assert_eq!(
            CommandIntent::classify("curl https://example.com"),
            CommandIntent::Network
        );
        assert_eq!(
            CommandIntent::classify("git push origin main"),
            CommandIntent::Network
        );
        assert_eq!(
            CommandIntent::classify("ssh host"),
            CommandIntent::Network
        );
    }

    #[test]
    fn test_classify_privilege_escalation() {
        assert_eq!(
            CommandIntent::classify("sudo apt install foo"),
            CommandIntent::PrivilegeEscalation
        );
        assert_eq!(
            CommandIntent::classify("su root"),
            CommandIntent::PrivilegeEscalation
        );
        assert_eq!(
            CommandIntent::classify("doas pacman -Syu"),
            CommandIntent::PrivilegeEscalation
        );
    }

    #[test]
    fn test_validate_allows_readonly() {
        assert_eq!(validate_command("ls"), ValidationResult::Allow);
    }

    #[test]
    fn test_validate_blocks_privilege_escalation() {
        assert!(matches!(
            validate_command("sudo rm -rf /"),
            ValidationResult::Block(_)
        ));
    }

    #[test]
    fn test_validate_requires_approval_for_destructive() {
        assert!(matches!(
            validate_command("rm -rf /tmp/foo"),
            ValidationResult::RequireApproval(_)
        ));
    }

    #[test]
    fn test_validate_requires_approval_for_network() {
        assert!(matches!(
            validate_command("curl https://evil.com"),
            ValidationResult::RequireApproval(_)
        ));
    }
}
