#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W9 of `docs/plans/worker-tools-spec.md`: `AgentHandle::mode()`.
//! New API: this file fails to compile until it exists.

mod worker_tools_support;

use hone_core::{AgentHandle, AgentMode};

use worker_tools_support::*;

fn handle_in(mode: AgentMode, dir: &std::path::Path) -> AgentHandle {
    let provider = ScriptedProvider::new(&[]);
    let mut config = lead_config(provider, session_tools(&new_log()), dir);
    config.mode = mode;
    AgentHandle::spawn(config)
}

#[tokio::test]
async fn test_w9_mode_when_freshly_spawned_should_return_the_configs_mode() {
    let dir = tempfile::tempdir().unwrap();
    for mode in [AgentMode::Build, AgentMode::Plan, AgentMode::AutoApprove] {
        let handle = handle_in(mode, dir.path());
        assert_eq!(handle.mode().await, Some(mode));
    }
}

#[tokio::test]
async fn test_w9_mode_after_set_mode_should_return_the_last_mode_set() {
    let dir = tempfile::tempdir().unwrap();
    let handle = handle_in(AgentMode::Build, dir.path());

    handle.set_mode(AgentMode::Plan).await;
    assert_eq!(handle.mode().await, Some(AgentMode::Plan));

    handle.set_mode(AgentMode::AutoApprove).await;
    assert_eq!(handle.mode().await, Some(AgentMode::AutoApprove));

    handle.set_mode(AgentMode::Build).await;
    assert_eq!(handle.mode().await, Some(AgentMode::Build));
}

#[tokio::test]
async fn test_w9_mode_when_actor_has_stopped_should_return_none() {
    let dir = tempfile::tempdir().unwrap();
    let handle = handle_in(AgentMode::Build, dir.path());

    handle.clone().shutdown().await;

    assert_eq!(handle.mode().await, None);
}
