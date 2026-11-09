//! Loading the permission policy for CLI runs (P2.2 spec section 7).
//!
//! The implementation lives in `hone_tools::policy_gate` (P2.3); this module
//! only re-exports it for the CLI.

pub(crate) use hone_tools::policy_gate::resolve_permission_policy;

#[cfg(test)]
#[path = "policy_load_tests.rs"]
mod tests;
