/// Tool approval policy for the agent loop.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolApprovalPolicy {
    /// Never pause — used in headless/CI/one-shot mode.
    Auto,
    /// Pause for *every* tool call.
    AlwaysAsk,
    /// Auto-approve read-only tools; pause for risky (write/shell) ones.
    AskRisky {
        /// Tool names approved for the entire session.
        always_allowed: Vec<String>,
    },
}

impl Default for ToolApprovalPolicy {
    fn default() -> Self {
        ToolApprovalPolicy::AskRisky {
            always_allowed: Vec::new(),
        }
    }
}

impl ToolApprovalPolicy {
    /// Returns `true` when the agent should pause and ask the user before
    /// executing a tool call.
    pub fn requires_approval(&self, tool_name: &str, is_read_only: bool) -> bool {
        match self {
            Self::Auto => false,
            Self::AlwaysAsk => true,
            Self::AskRisky { always_allowed } => {
                if is_read_only {
                    return false;
                }
                !always_allowed.iter().any(|n| n == tool_name)
            }
        }
    }

    /// Record that the user has approved `tool_name` for the rest of the session.
    ///
    /// A no-op for `Auto` and `AlwaysAsk` since they don't maintain an allowlist.
    pub fn approve_always(&mut self, tool_name: &str) {
        if let Self::AskRisky { always_allowed } = self {
            if !always_allowed.iter().any(|n| n == tool_name) {
                always_allowed.push(tool_name.to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ToolApprovalPolicy::Auto
    // -----------------------------------------------------------------------

    #[test]
    fn auto_never_requires_approval_for_write_tool() {
        // Auto should never ask for approval — even for write/shell tools.
        let policy = ToolApprovalPolicy::Auto;
        assert!(!policy.requires_approval("shell", false));
    }

    #[test]
    fn auto_never_requires_approval_for_read_only_tool() {
        let policy = ToolApprovalPolicy::Auto;
        assert!(!policy.requires_approval("read_file", true));
    }

    // -----------------------------------------------------------------------
    // ToolApprovalPolicy::AlwaysAsk
    // -----------------------------------------------------------------------

    #[test]
    fn always_ask_requires_approval_for_write_tool() {
        let policy = ToolApprovalPolicy::AlwaysAsk;
        assert!(policy.requires_approval("shell", false));
    }

    #[test]
    fn always_ask_requires_approval_for_read_only_tool() {
        // AlwaysAsk pauses for every tool, including read-only ones.
        let policy = ToolApprovalPolicy::AlwaysAsk;
        assert!(policy.requires_approval("read_file", true));
    }

    // -----------------------------------------------------------------------
    // ToolApprovalPolicy::AskRisky (default — empty always_allowed)
    // -----------------------------------------------------------------------

    #[test]
    fn risky_skips_read_only_tools() {
        // Read-only tools must not require approval under AskRisky.
        let policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        assert!(!policy.requires_approval("read_file", true));
    }

    #[test]
    fn risky_pauses_on_write_tools() {
        // Write/shell tools must require approval under AskRisky.
        let policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        assert!(policy.requires_approval("shell.run", false));
    }

    #[test]
    fn risky_remembers_approve_always() {
        // After approve_always("shell.run"), that specific tool no longer needs
        // approval; other write tools still do.
        let mut policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        policy.approve_always("shell.run");
        assert!(
            !policy.requires_approval("shell.run", false),
            "shell.run should no longer require approval after approve_always"
        );
        assert!(
            policy.requires_approval("write_file", false),
            "write_file should still require approval"
        );
    }

    #[test]
    fn approve_always_idempotent() {
        // Calling approve_always twice must not duplicate the entry.
        let mut policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        policy.approve_always("shell.run");
        policy.approve_always("shell.run");

        // Inspect always_allowed directly — it must have exactly one entry.
        let ToolApprovalPolicy::AskRisky { always_allowed } = &policy else {
            panic!("expected AskRisky variant");
        };
        let occurrences = always_allowed
            .iter()
            .filter(|n| n.as_str() == "shell.run")
            .count();
        assert_eq!(
            occurrences, 1,
            "shell.run must appear exactly once in always_allowed; got {occurrences}"
        );
    }
}
