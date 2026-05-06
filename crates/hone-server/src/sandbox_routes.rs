//! `GET /sandbox/policies` — read-only viewer for the server-wide
//! sandbox policy plus any per-agent overrides declared in agent
//! definitions.
//!
//! The `hone_sandbox::SandboxPolicy` struct does not derive `Serialize`,
//! so we mirror it into a wire shape (`SandboxPolicyWire`) here. That
//! also gives us a stable place to add GUI-only fields like
//! `description` later.

use std::path::Path;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use hone_core::agent_def::SandboxProfile;
use hone_core::agent_registry::AgentRegistry;
use hone_sandbox::{SandboxMode, SandboxPolicy};
use serde::Serialize;

use crate::state::AppState;

/// Wire shape of `hone_sandbox::SandboxPolicy`. Mirrored verbatim with
/// the addition of an optional `description` for the GUI to render the
/// active policy's intent.
#[derive(Debug, Clone, Serialize)]
pub struct SandboxPolicyWire {
    pub mode: SandboxMode,
    pub allow_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub allow_exec: Vec<String>,
    pub allow_network: bool,
    pub allowed_domains: Vec<String>,
    pub allow_env: Vec<String>,
    /// Optional human-readable summary the GUI surfaces above the rule
    /// table. Only the active server policy carries one today.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl SandboxPolicyWire {
    fn from_policy(p: &SandboxPolicy, description: Option<String>) -> Self {
        Self {
            mode: p.mode,
            allow_read: p.allow_read.iter().map(|p| p.display().to_string()).collect(),
            allow_write: p.allow_write.iter().map(|p| p.display().to_string()).collect(),
            allow_exec: p.allow_exec.iter().map(|p| p.display().to_string()).collect(),
            allow_network: p.allow_network,
            allowed_domains: p.allowed_domains.clone(),
            allow_env: p.allow_env.clone(),
            description,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSandboxOverride {
    pub agent: String,
    pub policy: SandboxPolicyWire,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxPoliciesResponse {
    pub active: SandboxPolicyWire,
    pub agent_overrides: Vec<AgentSandboxOverride>,
}

/// `GET /sandbox/policies`
pub async fn sandbox_policies_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    Json(collect_policies(&state.workspace_root))
}

fn collect_policies(workspace_root: &Path) -> SandboxPoliciesResponse {
    // The active server-wide policy is the same `SandboxPolicy::permissive()`
    // wired into `execution::run_handler` (and `orchestration`). We pin
    // that here so the wire response stays in sync — if either handler
    // changes, this collector should be updated too.
    let active = SandboxPolicy::permissive();
    let active_wire = SandboxPolicyWire::from_policy(
        &active,
        Some(
            "Permissive — manual mode, network on, no allow-lists. \
             Agents with their own `sandbox:` block in frontmatter \
             override this."
                .to_string(),
        ),
    );

    let registry = AgentRegistry::load_hierarchical(workspace_root);
    let mut overrides: Vec<AgentSandboxOverride> = registry
        .list()
        .iter()
        .filter_map(|def| {
            def.sandbox.as_ref().map(|profile| AgentSandboxOverride {
                agent: def.name.clone(),
                policy: profile_to_wire(profile),
            })
        })
        .collect();
    overrides.sort_by(|a, b| a.agent.cmp(&b.agent));

    SandboxPoliciesResponse {
        active: active_wire,
        agent_overrides: overrides,
    }
}

fn profile_to_wire(profile: &SandboxProfile) -> SandboxPolicyWire {
    // Build the policy through the type's own constructor to capture any
    // future invariants, then convert.
    let policy = profile.to_policy();
    SandboxPolicyWire::from_policy(&policy, None)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collect_policies_always_returns_active_with_description() {
        let tmp = TempDir::new().expect("tmp");
        let resp = collect_policies(tmp.path());
        assert!(resp.active.description.is_some(), "active must carry description");
        // Permissive policy has network on and empty allow-lists.
        assert!(resp.active.allow_network);
        assert!(resp.active.allow_read.is_empty());
        assert!(resp.active.allow_write.is_empty());
    }

    #[test]
    fn collect_policies_agent_overrides_are_sorted_by_name() {
        let tmp = TempDir::new().expect("tmp");
        let resp = collect_policies(tmp.path());
        for pair in resp.agent_overrides.windows(2) {
            assert!(pair[0].agent <= pair[1].agent, "must be sorted by agent name");
        }
    }

    #[test]
    fn sandbox_policy_wire_round_trips_paths_via_display() {
        let tmp = TempDir::new().expect("tmp");
        let policy = hone_sandbox::SandboxPolicyBuilder::new(SandboxMode::Manual)
            .allow_read(tmp.path().to_path_buf())
            .build()
            .expect("build");
        let wire = SandboxPolicyWire::from_policy(&policy, None);
        assert_eq!(wire.allow_read.len(), 1);
        assert!(wire.allow_read[0].contains(&tmp.path().display().to_string()));
        assert!(wire.description.is_none());
    }
}
