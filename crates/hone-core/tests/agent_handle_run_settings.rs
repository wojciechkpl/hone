#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W32 of `docs/plans/worker-tools-spec.md`: `AgentHandle::run_settings() ->
//! Option<RunSettings>`: approval policy, mode and session approvals in one
//! actor round trip; `None` once the actor has stopped. New API: this file fails
//! to compile until it exists. Assumed: fields `approval_policy: ToolApprovalPolicy`,
//! `mode: AgentMode`, `session_approvals: Arc<SessionApprovals>`.

mod worker_tools_support;

use std::sync::Arc;

use hone_core::{AgentHandle, AgentMode};
use hone_proto::ToolApprovalPolicy;

use worker_tools_support::*;

fn handle(mode: AgentMode, policy: ToolApprovalPolicy, dir: &std::path::Path) -> AgentHandle {
    let mut config = lead_config(ScriptedProvider::new(&[]), session_tools(&new_log()), dir);
    config.mode = mode;
    config.approval_policy = policy;
    AgentHandle::spawn(config)
}

#[tokio::test]
async fn test_w32_run_settings_when_freshly_spawned_should_return_the_configs_values() {
    let dir = tempfile::tempdir().unwrap();
    let h = handle(AgentMode::Plan, ToolApprovalPolicy::AlwaysAsk, dir.path());

    let s = h.run_settings().await.expect("actor running");

    assert_eq!(s.approval_policy, ToolApprovalPolicy::AlwaysAsk);
    assert_eq!(s.mode, AgentMode::Plan);
}

#[tokio::test]
async fn test_w32_run_settings_should_reflect_later_changes() {
    let dir = tempfile::tempdir().unwrap();
    let h = handle(
        AgentMode::Build,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        },
        dir.path(),
    );

    h.set_mode(AgentMode::AutoApprove).await;
    h.approve_tools(vec!["shell".to_owned()]).await;
    let s = h.run_settings().await.expect("actor running");

    assert_eq!(s.mode, AgentMode::AutoApprove);
    assert_eq!(
        s.approval_policy,
        ToolApprovalPolicy::AskRisky {
            always_allowed: vec!["shell".to_owned()]
        }
    );
}

#[tokio::test]
async fn test_w32_run_settings_should_carry_the_same_session_approvals_as_the_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let h = handle(AgentMode::Build, ToolApprovalPolicy::Auto, dir.path());

    let s = h.run_settings().await.expect("actor running");
    let a = h.session_approvals().await.expect("actor running");

    assert!(Arc::ptr_eq(&s.session_approvals, &a));
}

#[tokio::test]
async fn test_w32_run_settings_when_actor_stopped_should_be_none() {
    let dir = tempfile::tempdir().unwrap();
    let h = handle(AgentMode::Build, ToolApprovalPolicy::Auto, dir.path());
    h.clone().shutdown().await;

    assert!(h.run_settings().await.is_none());
}
