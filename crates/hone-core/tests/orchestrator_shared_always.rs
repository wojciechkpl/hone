#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W16 of `docs/plans/worker-tools-spec.md`: the workers of one run share their
//! "always" answers (default orchestrator, no new API needed).

mod worker_tools_support;

use std::sync::Arc;

use hone_config::Decision;
use hone_core::Orchestrator;
use hone_proto::{StepAction, ToolApprovalPolicy};
use serde_json::json;

use worker_tools_support::*;

fn always() -> Answerer {
    Box::new(|_| StepAction::ApproveAlways("shell".to_owned()))
}

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

/// Two parallel workers that each call `shell` once.
fn two_shell_workers(provider: &ScriptedProvider) {
    provider.script(
        "SA-A",
        vec![tool_call("sa1", "shell", json!({ "who": "a" }))],
    );
    provider.script(
        "SA-B",
        vec![tool_call("sb1", "shell", json!({ "who": "b" }))],
    );
}

#[tokio::test]
async fn test_w16_parallel_workers_when_one_is_answered_always_should_ask_only_once() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["SA-A", "SA-B"]);
    two_shell_workers(&provider);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    // The default orchestrator (no with_session_approvals) shares between its
    // own workers. The second worker is queued on the step channel while the
    // first is asked, so it must re-check once it holds the channel.
    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "SA-A", "rust-expert"),
            subtask("2", "SA-B", "rust-expert"),
        ]),
        Some(step_tx),
        always(),
    )
    .await;

    assert_eq!(
        paused_ids(&out.events).len(),
        1,
        "exactly one prompt for shell: {:?}",
        out.events
    );
    assert_eq!(invoked(&log), vec!["shell".to_owned(), "shell".to_owned()]);
}

#[tokio::test]
async fn test_w16_later_wave_worker_should_not_ask_after_an_earlier_workers_always() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["SA-A", "SA-B"]);
    two_shell_workers(&provider);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    let mut second = subtask("2", "SA-B", "rust-expert");
    second.depends_on = vec!["1".to_owned()];

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "SA-A", "rust-expert"), second]),
        Some(step_tx),
        always(),
    )
    .await;

    assert_eq!(paused_ids(&out.events), vec!["sa1".to_owned()]);
    assert_eq!(invoked(&log), vec!["shell".to_owned(), "shell".to_owned()]);
}

#[tokio::test]
async fn test_w16_parallel_workers_when_policy_ask_rule_answered_always_should_ask_only_once() {
    let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["SA-A", "SA-B"]);
    two_shell_workers(&provider);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(
        Arc::clone(&provider),
        session_tools_with_shell_subject(&log, Some("deploy prod")),
        &dir,
    );
    config.approval_policy = ToolApprovalPolicy::Auto;
    config.step_rx = Some(step_rx);
    config.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "SA-A", "rust-expert"),
            subtask("2", "SA-B", "rust-expert"),
        ]),
        Some(step_tx),
        always(),
    )
    .await;

    assert_eq!(
        paused_ids(&out.events).len(),
        1,
        "the second worker's rule is cleared by the first's entries: {:?}",
        out.events
    );
    assert_eq!(invoked(&log), vec!["shell".to_owned(), "shell".to_owned()]);
}

#[tokio::test]
async fn test_w16_workers_when_policy_is_always_ask_should_ask_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["SA-A", "SA-B"]);
    two_shell_workers(&provider);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ToolApprovalPolicy::AlwaysAsk;
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "SA-A", "rust-expert"),
            subtask("2", "SA-B", "rust-expert"),
        ]),
        Some(step_tx),
        always(),
    )
    .await;

    assert_eq!(paused_ids(&out.events).len(), 2, "--step asks every time");
}

#[tokio::test]
async fn test_w16_workers_when_always_is_for_another_tool_should_still_ask() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["SA-A", "SA-B"]);
    provider.script("SA-A", vec![tool_call("sa1", "shell", json!({}))]);
    provider.script("SA-B", vec![tool_call("sb1", "edit_file", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "SA-A", "rust-expert"),
            subtask("2", "SA-B", "rust-expert"),
        ]),
        Some(step_tx),
        always(),
    )
    .await;

    assert_eq!(paused_ids(&out.events).len(), 2);
}
