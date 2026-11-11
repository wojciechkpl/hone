#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W33 of `docs/plans/worker-tools-spec.md`, core side: an orchestrator
//! worker's approval events carry its `WorkerRef { task_id, agent }`; the main
//! agent's carry `None`. Assumed APIs: `hone_proto::WorkerRef`, and
//! `Agent::with_worker(self, WorkerRef) -> Self` (the orchestrator tags each
//! worker's `Agent` with it). Fails to compile until they exist.

mod worker_tools_support;

use std::sync::Arc;

use hone_config::Decision;
use hone_core::{Agent, Orchestrator};
use hone_proto::{AgentEvent, StepAction, ToolApprovalPolicy, WorkerRef};
use serde_json::json;

use worker_tools_support::*;

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

fn paused_workers(events: &[AgentEvent]) -> Vec<(String, Option<WorkerRef>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallPaused { id, worker, .. } => Some((id.clone(), worker.clone())),
            _ => None,
        })
        .collect()
}

fn wref(task: &str, agent: &str) -> Option<WorkerRef> {
    Some(WorkerRef {
        task_id: task.to_owned(),
        agent: agent.to_owned(),
    })
}

#[tokio::test(start_paused = true)]
async fn test_w33_each_workers_pause_should_carry_its_own_task_id_and_agent() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["WR-A", "WR-B"]);
    provider.script("WR-A", vec![tool_call("wa1", "shell", json!({}))]);
    provider.script("WR-B", vec![tool_call("wb1", "shell", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "WR-A", "rust-expert"),
            subtask("2", "WR-B", "code-reviewer"),
        ]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    let mut got = paused_workers(&out.events);
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        vec![
            ("wa1".to_owned(), wref("1", "rust-expert")),
            ("wb1".to_owned(), wref("2", "code-reviewer")),
        ]
    );
}

#[tokio::test]
async fn test_w33_workers_policy_prompt_should_tag_both_approval_required_and_pause() {
    let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["WR-POLICY"]);
    provider.script("WR-POLICY", vec![tool_call("wp1", "shell", json!({}))]);
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
        plan(vec![subtask("7", "WR-POLICY", "rust-expert")]),
        Some(step_tx),
        proceed_all(),
    )
    .await;

    let approvals: Vec<Option<WorkerRef>> = out
        .events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalRequired { worker, .. } => Some(worker.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(approvals, vec![wref("7", "rust-expert")]);
    assert_eq!(
        paused_workers(&out.events),
        vec![("wp1".to_owned(), wref("7", "rust-expert"))]
    );
}

#[tokio::test]
async fn test_w33_plain_agent_turn_events_should_carry_no_worker() {
    let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
    let log = new_log();
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(
        ScriptedProvider::new(&[]),
        session_tools_with_shell_subject(&log, Some("deploy prod")),
        &dir,
    );
    config.provider = CallsOnce::new(vec![tool_call("m1", "shell", json!({}))]);
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    config.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));
    let mut agent = Agent::new(config);

    let events = run_agent_turn(&mut agent, &step_tx, vec![StepAction::Proceed]).await;

    let mut seen = 0;
    for e in &events {
        match e {
            AgentEvent::ToolCallPaused { worker, .. }
            | AgentEvent::ApprovalRequired { worker, .. } => {
                assert_eq!(*worker, None);
                seen += 1;
            }
            _ => {}
        }
    }
    assert_eq!(seen, 2, "a policy prompt and its pause: {events:?}");
}
