//! Typed failure recovery recipes.
//!
//! Instead of blindly propagating tool and provider errors to the model,
//! classify common failures and attempt exactly ONE automatic recovery
//! before escalating. This reduces "agent gives up" failures.
//!
//! Source: claw-code RecoveryRecipes pattern (see priv/claw-code-review.md).

use std::time::Duration;

/// Classified failure scenarios with known recovery strategies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureScenario {
    /// Transient network error (connection refused, timeout, DNS failure).
    TransientNetwork,
    /// Provider rate limit hit — should back off and retry.
    RateLimit { retry_after_secs: Option<u64> },
    /// Git merge conflict during a commit/rebase/merge.
    MergeConflict,
    /// File not found when reading an expected file.
    FileNotFound { path: String },
    /// Test suite failed after a code edit.
    TestsFailed { output: String },
    /// Shell command exited with non-zero status.
    CommandFailed { exit_code: i32, stderr: String },
    /// Permission denied (sandbox rejected the operation).
    PermissionDenied { path: String },
    /// Unclassified error — no automatic recovery available.
    Unknown,
}

/// A recovery action to attempt after a classified failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Wait for a duration then retry the same operation.
    RetryAfter(Duration),
    /// Re-classify and fall through to the model (escalate).
    Escalate,
    /// Run a shell command to resolve the state, then retry.
    RunCommandThenRetry(String),
    /// Give up with a user-facing message.
    GiveUp(String),
}

impl FailureScenario {
    /// Classify an error string into a known scenario.
    pub fn classify(error: &str) -> Self {
        let lower = error.to_lowercase();

        // Network errors
        if lower.contains("connection refused")
            || lower.contains("connection reset")
            || lower.contains("timed out")
            || lower.contains("dns")
            || lower.contains("network is unreachable")
        {
            return Self::TransientNetwork;
        }

        // Rate limits
        if lower.contains("rate limit")
            || lower.contains("429")
            || lower.contains("too many requests")
        {
            let retry_after = extract_retry_after(error);
            return Self::RateLimit {
                retry_after_secs: retry_after,
            };
        }

        // Merge conflicts
        if lower.contains("merge conflict")
            || lower.contains("unmerged paths")
            || lower.contains("conflict in")
            || lower.contains("fix conflicts")
        {
            return Self::MergeConflict;
        }

        // File not found
        if lower.contains("no such file") || lower.contains("not found") {
            let path = extract_path_from_error(error).unwrap_or_default();
            return Self::FileNotFound { path };
        }

        // Permission denied
        if lower.contains("permission denied")
            || lower.contains("eacces")
            || lower.contains("sandbox denied")
        {
            let path = extract_path_from_error(error).unwrap_or_default();
            return Self::PermissionDenied { path };
        }

        // Test failures
        if lower.contains("test failed")
            || lower.contains("tests failed")
            || lower.contains("failures:")
        {
            return Self::TestsFailed {
                output: error.to_string(),
            };
        }

        Self::Unknown
    }

    /// Return the recommended recovery action for this scenario.
    pub fn recommended_action(&self) -> RecoveryAction {
        match self {
            Self::TransientNetwork => RecoveryAction::RetryAfter(Duration::from_millis(500)),
            Self::RateLimit { retry_after_secs } => {
                let secs = retry_after_secs.unwrap_or(2);
                RecoveryAction::RetryAfter(Duration::from_secs(secs.max(1)))
            }
            Self::MergeConflict => RecoveryAction::GiveUp(
                "merge conflict detected — manual resolution required".to_string(),
            ),
            Self::FileNotFound { .. } => RecoveryAction::Escalate,
            Self::TestsFailed { .. } => RecoveryAction::Escalate,
            Self::CommandFailed { .. } => RecoveryAction::Escalate,
            Self::PermissionDenied { .. } => RecoveryAction::Escalate,
            Self::Unknown => RecoveryAction::Escalate,
        }
    }
}

fn extract_retry_after(error: &str) -> Option<u64> {
    let lower = error.to_lowercase();
    if let Some(idx) = lower.find("retry after") {
        let rest = &lower[idx + "retry after".len()..];
        let num: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        return num.parse().ok();
    }
    None
}

fn extract_path_from_error(error: &str) -> Option<String> {
    // Naive: look for quoted path or a path-like token
    if let Some(start) = error.find('\'') {
        if let Some(end) = error[start + 1..].find('\'') {
            return Some(error[start + 1..start + 1 + end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_network() {
        assert_eq!(
            FailureScenario::classify("connection refused"),
            FailureScenario::TransientNetwork
        );
        assert_eq!(
            FailureScenario::classify("request timed out"),
            FailureScenario::TransientNetwork
        );
    }

    #[test]
    fn test_classify_rate_limit() {
        match FailureScenario::classify("429 too many requests") {
            FailureScenario::RateLimit { .. } => {}
            other => panic!("expected RateLimit, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_rate_limit_with_retry_after() {
        match FailureScenario::classify("rate limit exceeded, retry after 30 seconds") {
            FailureScenario::RateLimit {
                retry_after_secs: Some(30),
            } => {}
            other => panic!("expected RateLimit with retry_after, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_merge_conflict() {
        assert_eq!(
            FailureScenario::classify("merge conflict in src/auth.rs"),
            FailureScenario::MergeConflict
        );
    }

    #[test]
    fn test_classify_file_not_found() {
        match FailureScenario::classify("no such file 'src/missing.rs'") {
            FailureScenario::FileNotFound { path } => assert_eq!(path, "src/missing.rs"),
            other => panic!("expected FileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_permission_denied() {
        match FailureScenario::classify("permission denied: '/etc/passwd'") {
            FailureScenario::PermissionDenied { .. } => {}
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(
            FailureScenario::classify("something weird happened"),
            FailureScenario::Unknown
        );
    }

    #[test]
    fn test_network_recovery_is_retry() {
        match FailureScenario::TransientNetwork.recommended_action() {
            RecoveryAction::RetryAfter(_) => {}
            other => panic!("expected RetryAfter, got {other:?}"),
        }
    }

    #[test]
    fn test_merge_conflict_gives_up() {
        match FailureScenario::MergeConflict.recommended_action() {
            RecoveryAction::GiveUp(_) => {}
            other => panic!("expected GiveUp, got {other:?}"),
        }
    }
}
