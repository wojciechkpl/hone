#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W24 of `docs/plans/worker-tools-spec.md`: the post-turn test run ends with
//! its turn. `TestVerificationLoop::run` is public, so the test drops it
//! mid-command exactly as Esc does. The marker is written by a grandchild.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hone_core::{Agent, AgentConfig, AgentMode, TestVerificationLoop};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn agent() -> Agent {
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    Agent::new(AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "x".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: PathBuf::from("."),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    })
}

#[tokio::test]
async fn test_w24_dropping_the_test_run_should_kill_the_test_commands_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let test_cmd = format!("(sleep 1; touch '{}') & sleep 5", marker.display());
    let verification = TestVerificationLoop::new(test_cmd, 1, dir.path().to_path_buf());
    let mut agent = agent();
    let (tx, _rx) = mpsc::channel(64);

    let run = verification.run(&mut agent, tx);
    let ended_early = tokio::time::timeout(Duration::from_millis(200), run)
        .await
        .is_err();
    assert!(
        ended_early,
        "the test command (sleep 5) must still be running"
    );

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !marker.exists(),
        "the test command's grandchild outlived the dropped run"
    );
}

#[tokio::test]
async fn test_w24_test_run_that_finishes_should_still_report_its_result() {
    let dir = tempfile::tempdir().unwrap();
    let verification = TestVerificationLoop::new("exit 0", 1, dir.path().to_path_buf());
    let mut agent = agent();
    let (tx, _rx) = mpsc::channel(64);

    let result = verification.run(&mut agent, tx).await.unwrap();

    assert!(matches!(result.final_status, hone_core::TestStatus::Passed));
}
