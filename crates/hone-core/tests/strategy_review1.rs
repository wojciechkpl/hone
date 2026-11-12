#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 1 of `docs/plans/strategy-registry-spec.md` in `hone-core`:
//! S11 (a project budget_ratio is a cap), S17 (trim-only keeps the phase tracker)
//! and the History half of S21 (receipts name the compactor). No new API: these
//! are behaviour tests. S13, S14 and the writer half of S21 (new API) are in
//! `strategy_review1_api.rs`.

mod receipts_support;

use std::sync::Arc;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_core::{Agent, estimate_text_tokens};
use hone_proto::{AgentEvent, ContextReceipt, Slot, sha256_hex};
use receipts_support::*;
use serde_json::json;
use tokio::sync::mpsc;

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn no_inject() -> Flags {
    Flags {
        no_inject: true,
        ..Default::default()
    }
}

fn resolve(user: Option<&str>, project: Option<&str>) -> Arc<ResolvedStrategies> {
    let u = user.map(|t| ConfigLayer::from_toml(USER, t).unwrap());
    let p = project.map(|t| ConfigLayer::from_toml(PROJECT, t).unwrap());
    let sel =
        StrategySelection::from_sources(u.as_ref(), p.as_ref(), &|_| None, &no_inject()).unwrap();
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &StrategyDeps::default())
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

async fn run_agent(
    s: Arc<ResolvedStrategies>,
    rounds: Vec<Vec<hone_proto::ToolCallRequest>>,
    prompt: &str,
) -> (Agent, Arc<Script>, Vec<AgentEvent>) {
    let provider = Script::new(rounds);
    let mut agent = Agent::new(config(provider.clone(), Opts::default())).with_strategies(s);
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(4096);
    agent.run_turn(prompt, tx).await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (agent, provider, events)
}

// ---- S11: a project budget_ratio is a cap ---------------------------------------------------------------

/// Tools offered on the last request after one huge read (~70k tokens): the model
/// `mock:test` has a 128k window and a default ratio of 0.5 (64k budget), so a
/// 0.5 budget forces the emergency call (no tools) and a 0.6 budget (76.8k) does not.
async fn last_request_tools(s: Arc<ResolvedStrategies>, reps: u64) -> usize {
    let (_, provider, _) = run_agent(
        s,
        vec![vec![call(
            "big",
            "read_file",
            json!({ "tag": "big", "reps": reps }),
        )]],
        "read it",
    )
    .await;
    provider.request(provider.request_count() - 1).tools.len()
}

#[tokio::test]
async fn test_s11_a_project_ratio_above_the_model_default_should_not_raise_the_budget() {
    let reps = 14_000;
    assert!(
        estimate_text_tokens(&long_output("big", reps as usize)) > 64_000,
        "the scenario overshoots the default 64k budget"
    );

    let control_default = last_request_tools(resolve(None, None), reps).await;
    assert_eq!(
        control_default, 0,
        "control: the default 0.5 budget forces the emergency call"
    );
    let control_user = last_request_tools(resolve(Some("[context]\nbudget_ratio = 0.6\n"), None), reps).await;
    assert!(
        control_user > 0,
        "control: a USER 0.6 really raises the budget past this read"
    );

    let project = last_request_tools(resolve(None, Some("[context]\nbudget_ratio = 0.6\n")), reps).await;
    assert_eq!(
        project, 0,
        "a project 0.6 is a cap: min(0.5, 0.6) = 0.5, the same as no project value"
    );
}

#[tokio::test]
async fn test_s11_a_project_ratio_below_the_default_should_still_lower_the_budget() {
    // ~45k tokens fit the default 64k budget but not 0.3 * 128k = 38.4k.
    let reps = 9_000;
    assert!(last_request_tools(resolve(None, None), reps).await > 0, "control");

    let lowered = last_request_tools(resolve(None, Some("[context]\nbudget_ratio = 0.3\n")), reps).await;

    assert_eq!(lowered, 0, "a project file can lower the budget");
}

#[tokio::test]
async fn test_s11_the_effective_ratio_should_be_the_smaller_of_user_and_project() {
    // user 0.6 + project 0.5: min = 0.5 -> the 70k read overshoots.
    let reps = 14_000;
    let s = resolve(Some("[context]\nbudget_ratio = 0.6\n"), Some("[context]\nbudget_ratio = 0.5\n"));
    assert_eq!(last_request_tools(s, reps).await, 0);
    // user 0.5 + project 0.6: min = 0.5, the project cannot lift the user's value.
    let s = resolve(Some("[context]\nbudget_ratio = 0.5\n"), Some("[context]\nbudget_ratio = 0.6\n"));
    assert_eq!(last_request_tools(s, reps).await, 0);
}

// ---- S17: trim-only keeps the phase tracker -------------------------------------------------------------------

fn reads_then_edit() -> Vec<Vec<hone_proto::ToolCallRequest>> {
    // r1 is older than the recent window (4 for `mock:test`).
    let mut rounds: Vec<_> = (1..=6).map(|i| vec![read(&format!("r{i}"))]).collect();
    rounds.push(vec![call("e1", "edit_file", json!({ "tag": "e1" }))]);
    rounds
}

fn tool_texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .filter(|m| m.role == hone_proto::Role::Tool)
        .map(|m| m.content.as_text())
        .collect()
}

#[tokio::test]
async fn test_s17_trim_only_should_still_record_phase_transitions() {
    for toml in [
        "[context]\ncompactor = \"trim-only\"\n",
        "[context]\ncompactor = \"trim-only\"\n[context.graduated]\nphase = true\n",
    ] {
        let (agent, _, _) = run_agent(resolve(Some(toml), None), reads_then_edit(), "fix it").await;

        assert_eq!(
            agent.metrics_snapshot().phase_transitions,
            1,
            "the tracker runs under trim-only: {toml:?}"
        );
        assert!(
            tool_texts(&agent)[0].contains("lorem"),
            "...but nothing is compacted: {toml:?}"
        );
    }
}

// ---- S21: receipts name the compactor and the writer ------------------------------------------------------------

fn first_receipt(events: &[AgentEvent]) -> ContextReceipt {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .expect("a receipt")
}

#[tokio::test]
async fn test_s21_the_history_block_should_name_the_compactor_with_the_phase_param() {
    for (toml, name, params) in [
        ("", "graduated", r#"{"phase":false}"#),
        ("[context.graduated]\nphase = true\n", "graduated", r#"{"phase":true}"#),
        ("[context]\ncompactor = \"trim-only\"\n", "trim-only", r#"{"phase":false}"#),
    ] {
        let (_, _, events) = run_agent(resolve(Some(toml), None), vec![], "hello").await;

        let receipt = first_receipt(&events);
        let history = receipt
            .blocks
            .iter()
            .find(|b| b.slot == Slot::History)
            .expect("a History block");
        assert_eq!(history.strategy.name, name, "{toml:?}");
        assert_eq!(
            history.strategy.params_sha256,
            sha256_hex(params.as_bytes()),
            "{toml:?}: params {params}"
        );
    }
}

