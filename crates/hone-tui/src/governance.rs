use hone_proto::ToolApprovalPolicy;

/// Per-run governance overrides set by the user via :budget / :permit.
/// Drained at the start of the next run; not persisted.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PendingGovernance {
    pub budget_usd: Option<f64>,
    pub permits: Vec<String>,
}

impl PendingGovernance {
    /// Apply the pending budget override to a config field. Only writes when
    /// `budget_usd` is `Some` — a no-op pending struct must not clobber a
    /// limit set elsewhere (e.g. via the agent's TOML config).
    pub fn apply_to_budget(self, opt: &mut Option<f64>) {
        if let Some(usd) = self.budget_usd {
            *opt = Some(usd);
        }
    }

    /// Apply each pending permit via `ToolApprovalPolicy::approve_always`.
    /// No-op for `Auto` and `AlwaysAsk` (those variants don't carry an
    /// allowlist).
    pub fn apply_to_policy(self, policy: &mut ToolApprovalPolicy) {
        for tool in self.permits {
            policy.approve_always(&tool);
        }
    }
}
