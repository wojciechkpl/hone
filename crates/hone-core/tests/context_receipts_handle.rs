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
