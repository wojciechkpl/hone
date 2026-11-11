#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W13-W16 of `docs/plans/worker-tools-spec.md` (shared "always"), the parts
//! that need the new APIs: `SessionApprovals`, `Agent::with_session_approvals`,
//! `AgentHandle::session_approvals`, `Orchestrator::with_session_approvals`.
//! This file fails to compile until they exist.
//!
//! Interface assumptions: `SessionApprovals: Default`;
//! `Agent::with_session_approvals(self, Arc<SessionApprovals>) -> Self`;
//! `Orchestrator::with_session_approvals(self, Arc<SessionApprovals>) -> Self`.

mod worker_tools_support;

use std::sync::Arc;

use hone_config::Decision;
use hone_core::{Agent, AgentConfig, AgentHandle, Orchestrator, SessionApprovals};
use hone_proto::{StepAction, ToolApprovalPolicy};
use serde_json::json;

use worker_tools_support::*;

fn always() -> StepAction {
    StepAction::ApproveAlways("shell".to_owned())
}

/// A config whose provider issues one `shell` call (subject `shell_subject`)
/// and whose approval policy is `policy`, with its own step channel.
fn shell_config(
    dir: &std::path::Path,
    log: &InvocationLog,
    policy: ToolApprovalPolicy,
    shell_subject: Option<&'static str>,
) -> (AgentConfig, tokio::sync::mpsc::Sender<StepAction>) {
    let provider = CallsOnce::new(vec![tool_call("s1", "shell", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(
        ScriptedProvider::new(&[]),
        session_tools_with_shell_subject(log, shell_subject),
        dir,
    );
    config.provider = provider;
    config.approval_policy = policy;
    config.step_rx = Some(step_rx);
    (config, step_tx)
}

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

fn pauses(events: &[hone_proto::AgentEvent]) -> usize {
    paused_ids(events).len()
}

// ---------------------------------------------------------------------------
// W13
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w13_handles_should_have_their_own_session_approvals_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let a = AgentHandle::spawn(shell_config(dir.path(), &new_log(), ask_risky(), None).0);
    let b = AgentHandle::spawn(shell_config(dir.path(), &new_log(), ask_risky(), None).0);

    let sa1 = a.session_approvals().await.expect("actor running");
    let sa2 = a.session_approvals().await.expect("actor running");
    let sb = b.session_approvals().await.expect("actor running");

    assert!(Arc::ptr_eq(&sa1, &sa2), "stable for one agent");
    assert!(
        !Arc::ptr_eq(&sa1, &sb),
        "agents never share unless attached"
    );
}

#[tokio::test]
async fn test_w13_session_approvals_when_actor_stopped_should_be_none() {
    let dir = tempfile::tempdir().unwrap();
    let a = AgentHandle::spawn(shell_config(dir.path(), &new_log(), ask_risky(), None).0);
    a.clone().shutdown().await;
    assert!(a.session_approvals().await.is_none());
}

#[tokio::test]
async fn test_w13_agents_with_default_approvals_should_not_see_each_others_always() {
    let dir = tempfile::tempdir().unwrap();
    let log_a = new_log();
    let log_b = new_log();
    let (cfg_a, tx_a) = shell_config(dir.path(), &log_a, ask_risky(), None);
    let (cfg_b, tx_b) = shell_config(dir.path(), &log_b, ask_risky(), None);
    let mut a = Agent::new(cfg_a);
    let mut b = Agent::new(cfg_b);

    let ev_a = run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(pauses(&ev_a), 1);
    assert_eq!(pauses(&ev_b), 1, "B must still ask: A's always is A's own");
}

// ---------------------------------------------------------------------------
// W14 / W15: approval-policy pause (tool names)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w15_agent_when_another_recorded_always_for_tool_should_run_without_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let log_a = new_log();
    let log_b = new_log();
    let (cfg_a, tx_a) = shell_config(dir.path(), &log_a, ask_risky(), None);
    let (cfg_b, tx_b) = shell_config(dir.path(), &log_b, ask_risky(), None);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    let ev_a = run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    // No answers queued: any prompt from B fails the test.
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![]).await;

    assert_eq!(pauses(&ev_a), 1);
    assert_eq!(pauses(&ev_b), 0);
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()], "B's call ran");
}

#[tokio::test]
async fn test_w15_agent_when_policy_is_always_ask_should_still_prompt_for_a_recorded_tool() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let log_b = new_log();
    let (cfg_a, tx_a) = shell_config(dir.path(), &new_log(), ask_risky(), None);
    let (cfg_b, tx_b) = shell_config(dir.path(), &log_b, ToolApprovalPolicy::AlwaysAsk, None);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(pauses(&ev_b), 1, "--step asks every time");
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w15_agent_when_policy_is_auto_should_run_without_prompt_with_shared_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let log_b = new_log();
    let (cfg_a, tx_a) = shell_config(dir.path(), &new_log(), ask_risky(), None);
    let (cfg_b, tx_b) = shell_config(dir.path(), &log_b, ToolApprovalPolicy::Auto, None);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![]).await;

    assert_eq!(pauses(&ev_b), 0);
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w15_agent_when_recorded_always_is_for_another_tool_should_still_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let (cfg_a, tx_a) = shell_config(dir.path(), &new_log(), ask_risky(), None);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    // B calls edit_file (risky) instead of shell.
    let log_b = new_log();
    let (mut cfg_b, tx_b) = shell_config(dir.path(), &log_b, ask_risky(), None);
    cfg_b.provider = CallsOnce::new(vec![tool_call("e1", "edit_file", json!({}))]);
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(
        pauses(&ev_b),
        1,
        "always for shell does not cover edit_file"
    );
}

// ---------------------------------------------------------------------------
// W14 / W15: policy `ask` rule (entries)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w15_agent_when_another_recorded_always_for_ask_rule_should_run_without_prompt() {
    let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let log_b = new_log();
    let (mut cfg_a, tx_a) = shell_config(
        &dir,
        &new_log(),
        ToolApprovalPolicy::Auto,
        Some("deploy prod"),
    );
    let (mut cfg_b, tx_b) =
        shell_config(&dir, &log_b, ToolApprovalPolicy::Auto, Some("deploy prod"));
    cfg_a.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));
    cfg_b.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    let ev_a = run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![]).await;

    assert_eq!(pauses(&ev_a), 1);
    assert_eq!(pauses(&ev_b), 0, "B's policy gate sees A's entries");
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w15_agent_when_recorded_always_for_ask_rule_is_for_another_subject_should_prompt() {
    let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
    let shared = Arc::new(SessionApprovals::default());
    let (mut cfg_a, tx_a) = shell_config(
        &dir,
        &new_log(),
        ToolApprovalPolicy::Auto,
        Some("deploy prod"),
    );
    let (mut cfg_b, tx_b) = shell_config(
        &dir,
        &new_log(),
        ToolApprovalPolicy::Auto,
        Some("deploy staging"),
    );
    cfg_a.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));
    cfg_b.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(
        pauses(&ev_b),
        1,
        "approving 'deploy prod' is not 'deploy staging'"
    );
}

// ---------------------------------------------------------------------------
// W16: explicit session approvals on the orchestrator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w16_orchestrator_with_session_approvals_should_record_a_workers_always_there() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let shared = Arc::new(SessionApprovals::default());
    let provider = ScriptedProvider::new(&["W16-REC"]);
    provider.script("W16-REC", vec![tool_call("r1", "shell", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    let orch = Orchestrator::new(config, builtin_registry(), 4)
        .with_session_approvals(Arc::clone(&shared));
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W16-REC", "rust-expert")]),
        Some(step_tx),
        Box::new(|_| always()),
    )
    .await;
    assert_eq!(paused_ids(&out.events).len(), 1);

    // A plain agent holding the same approvals now runs shell unasked.
    let log_b = new_log();
    let (cfg_b, tx_b) = shell_config(dir.path(), &log_b, ask_risky(), None);
    let mut b = Agent::new(cfg_b).with_session_approvals(shared);
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![]).await;
    assert_eq!(pauses(&ev_b), 0);
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w16_orchestrator_when_passed_approvals_already_cover_the_tool_workers_should_not_ask()
{
    let dir = tempfile::tempdir().unwrap();
    let shared = Arc::new(SessionApprovals::default());
    // An agent records "always" for shell into `shared` first.
    let (cfg_a, tx_a) = shell_config(dir.path(), &new_log(), ask_risky(), None);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    run_agent_turn(&mut a, &tx_a, vec![always()]).await;

    let log = new_log();
    let provider = ScriptedProvider::new(&["W16-PRE"]);
    provider.script("W16-PRE", vec![tool_call("p1", "shell", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    let orch = Orchestrator::new(config, builtin_registry(), 4).with_session_approvals(shared);
    let out = run_plan_with_ui(
        orch,
        plan(vec![subtask("1", "W16-PRE", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    assert!(paused_ids(&out.events).is_empty(), "{:?}", out.events);
    assert_eq!(invoked(&log), vec!["shell".to_owned()]);
}
