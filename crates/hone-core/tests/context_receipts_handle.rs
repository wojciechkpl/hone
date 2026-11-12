#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR11 (agent side) of `docs/plans/context-receipts-spec.md`:
//! `AgentHandle::get_context_receipts().await -> Vec<hone_proto::ContextReceipt>`
//! returns the receipts of the agent's whole life, in order, like `get_messages`
//! (the newest 10,000 at most; the cap is not tested here). Receipt types: see
//! `crates/hone-proto/tests/receipt.rs`.

mod receipts_support;

use std::time::Duration;

use hone_proto::{AgentEvent, ContextReceipt};
use receipts_support::*;
use tokio::sync::mpsc;

async fn turn(h: &hone_core::AgentHandle, input: &str) -> Vec<ContextReceipt> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    tokio::time::timeout(Duration::from_secs(30), h.run_turn(input, tx))
        .await
        .expect("turn hung")
        .expect("turn failed");
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let AgentEvent::ContextAssembled { receipt } = e {
            out.push(receipt);
        }
    }
    out
}

#[tokio::test]
async fn test_cr11_handle_should_return_the_receipts_of_every_turn_in_order() {
    let (_provider, h) = handle(vec![vec![read("r1")]], Opts::default());

    let first = turn(&h, "one").await;
    let second = turn(&h, "two").await;

    let kept = h.get_context_receipts().await;
    let sent: Vec<ContextReceipt> = first.into_iter().chain(second).collect();
    assert_eq!(sent.len(), 3, "two calls in turn 1, one in turn 2");
    assert_eq!(kept, sent, "same receipts as the events, in the same order");
    let numbers: Vec<(u32, u32)> = kept.iter().map(|r| (r.turn, r.call)).collect();
    assert_eq!(numbers, [(1, 1), (1, 2), (2, 1)]);
}

#[tokio::test]
async fn test_cr11_a_fresh_handle_should_have_no_receipts() {
    let (_provider, h) = handle(vec![], Opts::default());
    assert!(h.get_context_receipts().await.is_empty());
}

// ---- CR21: ordinals and agent ids ----------------------------------------------------
//
// ASSUMED API: `hone_core::ReceiptBatch { pub agent_id: String, pub items: Vec<(u64, ContextReceipt)> }`
// and `AgentHandle::context_receipts_after(ordinal: u64).await -> ReceiptBatch`: the kept
// receipts with ordinals above `ordinal`; ordinals are 1-based for the agent's whole life.

#[tokio::test]
async fn test_cr21_context_receipts_after_zero_should_return_all_with_1_based_ordinals() {
    let (_provider, h) = handle(vec![vec![read("r1")]], Opts::default());
    turn(&h, "one").await;
    turn(&h, "two").await;

    let batch = h.context_receipts_after(0).await;

    let ordinals: Vec<u64> = batch.items.iter().map(|(o, _)| *o).collect();
    assert_eq!(ordinals, [1, 2, 3]);
    let receipts: Vec<ContextReceipt> = batch.items.iter().map(|(_, r)| r.clone()).collect();
    assert_eq!(
        receipts,
        h.get_context_receipts().await,
        "same receipts as the plain getter"
    );
}

#[tokio::test]
async fn test_cr21_context_receipts_after_n_should_return_only_newer_receipts() {
    let (_provider, h) = handle(vec![vec![read("r1")]], Opts::default());
    turn(&h, "one").await;
    turn(&h, "two").await;

    let after_two = h.context_receipts_after(2).await;
    let all = h.context_receipts_after(0).await;
    let beyond = h.context_receipts_after(3).await;

    assert_eq!(after_two.items.len(), 1);
    assert_eq!(after_two.items[0].0, 3);
    assert_eq!(after_two.items[0].1, all.items[2].1);
    assert_eq!(after_two.agent_id, all.agent_id);
    assert!(beyond.items.is_empty());
    assert!(h.context_receipts_after(99).await.items.is_empty());
}

#[tokio::test]
async fn test_cr21_ordinals_should_continue_across_turns_not_restart() {
    let (_provider, h) = handle(vec![vec![read("r1")]], Opts::default());
    turn(&h, "one").await;
    let first = h.context_receipts_after(0).await;
    turn(&h, "two").await;

    let more = h.context_receipts_after(first.items.len() as u64).await;

    assert_eq!(more.items.len(), 1);
    assert_eq!(more.items[0].0, first.items.len() as u64 + 1);
}

#[tokio::test]
async fn test_cr21_agent_id_should_be_stable_per_agent_and_differ_between_agents() {
    let (_p1, h1) = handle(vec![], Opts::default());
    let (_p2, h2) = handle(vec![], Opts::default());
    turn(&h1, "a").await;
    turn(&h1, "b").await;
    turn(&h2, "a").await;

    let first = h1.context_receipts_after(0).await.agent_id;
    let later = h1.context_receipts_after(1).await.agent_id;
    let other = h2.context_receipts_after(0).await.agent_id;

    assert!(!first.is_empty());
    assert_eq!(first, later, "one id for the agent's whole life");
    assert_ne!(first, other, "a fresh agent has its own id");
}

#[tokio::test]
async fn test_cr21_a_fresh_handle_should_return_an_empty_batch() {
    let (_provider, h) = handle(vec![], Opts::default());
    assert!(h.context_receipts_after(0).await.items.is_empty());
}
