#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W28 and W29 of `docs/plans/worker-tools-spec.md` (review round 2). Uses only
//! existing APIs, so the reds are real assertion failures. Every test here runs
//! under `AskRisky` (the TUI default); the earlier policy tests used `Auto`.

mod worker_tools_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hone_config::Decision;
use hone_core::{Agent, AgentConfig, Orchestrator, SessionApprovals};
use hone_proto::{AgentEvent, StepAction, ToolApprovalPolicy};
use serde_json::json;

use worker_tools_support::*;

fn ask_risky() -> ToolApprovalPolicy {
    ToolApprovalPolicy::AskRisky {
        always_allowed: vec![],
    }
}

fn always() -> StepAction {
    StepAction::ApproveAlways("shell".to_owned())
}

fn pauses(events: &[AgentEvent]) -> usize {
    paused_ids(events).len()
}

fn policy_prompts(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalRequired { .. }))
        .count()
}

/// An agent whose provider issues `calls` shell calls (ids s1, s2, ...), with a
/// policy `ask` rule on `deploy*` when `ask_rule`.
fn agent_cfg(
    dir: &std::path::Path,
    log: &InvocationLog,
    policy: ToolApprovalPolicy,
    ask_rule: bool,
    calls: usize,
) -> (AgentConfig, tokio::sync::mpsc::Sender<StepAction>) {
    let (step_tx, step_rx) = step_channel();
    let subject = ask_rule.then_some("deploy prod");
    let mut c = lead_config(
        ScriptedProvider::new(&[]),
        session_tools_with_shell_subject(log, subject),
        dir,
    );
    c.provider = CallsOnce::new(
        (1..=calls)
            .map(|i| tool_call(&format!("s{i}"), "shell", json!({})))
            .collect(),
    );
    c.approval_policy = policy;
    c.step_rx = Some(step_rx);
    if ask_rule {
        c.policy = Some(rule_policy(dir, "deploy*", Decision::Ask, "ask-deploy"));
    }
    (c, step_tx)
}

fn canon_dir() -> std::path::PathBuf {
    std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap()
}

// ---------------------------------------------------------------------------
// W28: an "always" at a policy prompt also covers the risky-tool pause
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w28_ask_risky_agent_when_another_answered_always_at_policy_prompt_should_not_prompt_at_all()
 {
    let dir = canon_dir();
    let shared = Arc::new(SessionApprovals::default());
    let (cfg_a, tx_a) = agent_cfg(&dir, &new_log(), ask_risky(), true, 1);
    let log_b = new_log();
    let (cfg_b, tx_b) = agent_cfg(&dir, &log_b, ask_risky(), true, 1);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    let ev_a = run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(pauses(&ev_a), 1, "A answers one prompt");
    assert_eq!(policy_prompts(&ev_b), 0, "no policy prompt for B");
    assert_eq!(pauses(&ev_b), 0, "...and no AskRisky menu either: {ev_b:?}");
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w28_ask_risky_agent_when_its_own_earlier_always_at_policy_prompt_should_not_pause_again()
 {
    let dir = canon_dir();
    let log = new_log();
    // Two shell calls in one turn; the first is answered "always".
    let (cfg, tx) = agent_cfg(&dir, &log, ask_risky(), true, 2);
    let mut agent = Agent::new(cfg);

    let ev = run_agent_turn(&mut agent, &tx, vec![always(), StepAction::Proceed]).await;

    assert_eq!(pauses(&ev), 1, "only the first call asks: {ev:?}");
    assert_eq!(invoked(&log), vec!["shell".to_owned(), "shell".to_owned()]);
}

#[tokio::test]
async fn test_w28_always_ask_agent_when_policy_cleared_by_session_always_should_still_pause() {
    let dir = canon_dir();
    let shared = Arc::new(SessionApprovals::default());
    let (cfg_a, tx_a) = agent_cfg(&dir, &new_log(), ask_risky(), true, 1);
    let log_b = new_log();
    let (cfg_b, tx_b) = agent_cfg(&dir, &log_b, ToolApprovalPolicy::AlwaysAsk, true, 1);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(policy_prompts(&ev_b), 0, "the policy rule is cleared");
    assert_eq!(pauses(&ev_b), 1, "--step still asks every time");
    assert_eq!(invoked(&log_b), vec!["shell".to_owned()]);
}

#[tokio::test]
async fn test_w28_ask_risky_parallel_workers_when_policy_always_given_should_ask_only_once() {
    // AskRisky variant of the W16 policy test (which used Auto).
    let dir = canon_dir();
    let log = new_log();
    let provider = ScriptedProvider::new(&["R2-A", "R2-B"]);
    provider.script("R2-A", vec![tool_call("ra1", "shell", json!({}))]);
    provider.script("R2-B", vec![tool_call("rb1", "shell", json!({}))]);
    let (step_tx, step_rx) = step_channel();
    let mut config = lead_config(
        Arc::clone(&provider),
        session_tools_with_shell_subject(&log, Some("deploy prod")),
        &dir,
    );
    config.approval_policy = ask_risky();
    config.step_rx = Some(step_rx);
    config.policy = Some(rule_policy(&dir, "deploy*", Decision::Ask, "ask-deploy"));

    let orch = Orchestrator::new(config, builtin_registry(), 4);
    let out = run_plan_with_ui(
        orch,
        plan(vec![
            subtask("1", "R2-A", "rust-expert"),
            subtask("2", "R2-B", "rust-expert"),
        ]),
        Some(step_tx),
        Box::new(|_| always()),
    )
    .await;

    assert_eq!(pauses(&out.events), 1, "{:?}", out.events);
    assert_eq!(policy_prompts(&out.events), 1);
    assert_eq!(invoked(&log), vec!["shell".to_owned(), "shell".to_owned()]);
}

// ---------------------------------------------------------------------------
// W29: "always" is recorded before the step channel is released
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w29_always_ask_agent_answered_always_should_record_nothing_for_the_pause() {
    let dir = canon_dir();
    let shared = Arc::new(SessionApprovals::default());
    let (cfg_a, tx_a) = agent_cfg(&dir, &new_log(), ToolApprovalPolicy::AlwaysAsk, false, 1);
    let log_b = new_log();
    let (cfg_b, tx_b) = agent_cfg(&dir, &log_b, ask_risky(), false, 1);
    let mut a = Agent::new(cfg_a).with_session_approvals(Arc::clone(&shared));
    let mut b = Agent::new(cfg_b).with_session_approvals(Arc::clone(&shared));

    let ev_a = run_agent_turn(&mut a, &tx_a, vec![always()]).await;
    let ev_b = run_agent_turn(&mut b, &tx_b, vec![StepAction::Proceed]).await;

    assert_eq!(pauses(&ev_a), 1);
    assert_eq!(
        pauses(&ev_b),
        1,
        "an --step 'always' is not a session grant: B (AskRisky) still asks"
    );
}

/// Two agents sharing approvals and one step channel run concurrently on a
/// multi-thread runtime; every prompt is answered "always" at once. Only one
/// prompt may go out: the answering agent must record before it releases the
/// channel, so the other one's re-check always sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_w29_concurrent_agents_should_never_prompt_twice_after_an_always() {
    let iterations = 1000;
    let doubles = Arc::new(AtomicUsize::new(0));
    for _ in 0..iterations {
        let dir = tempfile::tempdir().unwrap();
        let shared = Arc::new(SessionApprovals::default());
        let (step_tx, step_rx) = step_channel();
        let mk = |step_rx: StepRx| {
            let mut c = lead_config(
                ScriptedProvider::new(&[]),
                session_tools(&new_log()),
                dir.path(),
            );
            c.provider = CallsOnce::new(vec![tool_call("s1", "shell", json!({}))]);
            c.approval_policy = ask_risky();
            c.step_rx = Some(step_rx);
            c
        };
        let (ca, cb) = (mk(Arc::clone(&step_rx)), mk(step_rx));
        let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
        let (ea, eb) = (ev_tx.clone(), ev_tx);
        let (sa, sb) = (Arc::clone(&shared), Arc::clone(&shared));
        let ta = tokio::spawn(async move {
            let mut a = Agent::new(ca).with_session_approvals(sa);
            a.run_turn("go", ea).await
        });
        let tb = tokio::spawn(async move {
            let mut b = Agent::new(cb).with_session_approvals(sb);
            b.run_turn("go", eb).await
        });
        let mut prompts = 0;
        while let Some(e) = ev_rx.recv().await {
            if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                prompts += 1;
                step_tx.send(always()).await.unwrap();
            }
        }
        ta.await.unwrap().unwrap();
        tb.await.unwrap().unwrap();
        if prompts > 1 {
            doubles.fetch_add(1, Ordering::SeqCst);
        }
    }
    assert_eq!(
        doubles.load(Ordering::SeqCst),
        0,
        "runs (of {iterations}) that sent a second prompt after an 'always'"
    );
}
