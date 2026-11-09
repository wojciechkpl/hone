//! `--approve <RULE_ID>` validation and provider gating (P2.3 spec sections 3
//! and 5). The logic is shared with honed and lives in `hone_tools::policy_gate`.

pub(crate) use hone_tools::policy_gate::{check_provider_gating, validate_approvals};

#[cfg(test)]
#[path = "approve_tests.rs"]
mod tests;
