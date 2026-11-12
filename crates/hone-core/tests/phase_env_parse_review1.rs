#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S18 of `docs/plans/strategy-registry-spec.md`, agent half: an agent built
//! WITHOUT strategies parses `HONE_PHASE_COMPACTION` like the selection does:
//! trimmed, case-insensitive `1` / `true` / `on`. (The selection half is in
//! `crates/hone-config/tests/strategy_review1.rs`.)
//!
//! One test that sets and removes the process-global variable sequentially, so
//! nothing races (the same arrangement as `phase_compaction_env_values.rs`).

mod receipts_support;

use hone_core::Agent;
use hone_proto::AgentEvent;
use receipts_support::*;
use serde_json::json;
use tokio::sync::mpsc;

/// Five reads then an edit through a real agent; true when the first read was summarised.
async fn compacted() -> bool {
    // r1 is older than the recent window (4 for `mock:test`) but younger than graduated
    // compression (5 assistant turns), so only phase compaction summarises it.
    let mut rounds: Vec<_> = (1..=5).map(|i| vec![read(&format!("r{i}"))]).collect();
    rounds.push(vec![call("e1", "edit_file", json!({ "tag": "e1" }))]);
    let provider = Script::new(rounds);
    let mut agent = Agent::new(config(provider, Opts::default()));
    let (tx, _rx) = mpsc::channel::<AgentEvent>(4096);
    agent.run_turn("fix it", tx).await.unwrap();
    agent
        .messages()
        .iter()
        .filter(|m| m.role == hone_proto::Role::Tool)
        .map(|m| m.content.as_text())
        .next()
        .unwrap()
        .starts_with("[tool result summarized")
}

#[tokio::test]
async fn test_s18_an_agent_without_strategies_should_parse_the_env_var_like_the_selection() {
    // SAFETY: the only test of this binary, so no other thread touches the environment.
    unsafe { std::env::remove_var("HONE_PHASE_COMPACTION") };
    assert!(!compacted().await, "unset: off");

    for on in ["1", " 1", "1 ", "true", "TRUE", "True", "on", "On", "ON", " on "] {
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", on) };
        assert!(compacted().await, "HONE_PHASE_COMPACTION={on:?} enables phase compaction");
    }
    for off in ["0", "false", "off", "", "yes", "2", " "] {
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", off) };
        assert!(!compacted().await, "HONE_PHASE_COMPACTION={off:?} leaves it off");
    }
    unsafe { std::env::remove_var("HONE_PHASE_COMPACTION") };
}
