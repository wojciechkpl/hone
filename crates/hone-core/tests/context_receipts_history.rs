#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR15 of `docs/plans/context-receipts-spec.md` (review round 1): the `History`
//! block lists only the non-system messages that are new or changed since the
//! agent's previous receipt (by hash), each with its position in this request. The
//! first receipt lists all of them. Every receipt carries `history_len` (the number
//! of non-system messages sent) and `history_root` (SHA-256 hex of the newline-joined
//! per-message hashes in request order). This file replaces the old CR7 test, which
//! required every message on every call.
//!
//! API required (new): `ContextReceipt.history_len: u32` (or any integer type) and
//! `ContextReceipt.history_root: String`, serialised under those names. These tests
//! read them through `serde_json::to_value(receipt)`, so they fail individually
//! (not at compile time) until the fields exist.

mod receipts_support;

use std::collections::HashSet;
use std::sync::Arc;

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{
    AgentEvent, ContextReceipt, Message, Role, Slot, SourceRef, message_sha256, sha256_hex,
};
use receipts_support::*;
use serde_json::{Value, json};

fn receipts(events: &[AgentEvent]) -> Vec<ContextReceipt> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

fn field(r: &ContextReceipt, name: &str) -> Value {
    serde_json::to_value(r).unwrap()[name].clone()
}

/// (request position, hash) of every non-system message of a recorded request.
fn non_system(messages: &[Message]) -> Vec<(usize, String)> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != Role::System)
        .map(|(i, m)| (i, message_sha256(m)))
        .collect()
}

/// What the History block of a receipt lists: (position, hash).
fn listed(r: &ContextReceipt) -> Vec<(usize, String)> {
    r.blocks
        .iter()
        .filter(|b| b.slot == Slot::History)
        .flat_map(|b| &b.sources)
        .map(|s| match &s.source {
            SourceRef::Message { index, sha256 } => (*index, sha256.clone()),
            other => panic!("history lists messages: {other:?}"),
        })
        .collect()
}

/// Checks every receipt of `rs` against the requests the provider recorded:
/// the incremental listing, `history_len` and the exact `history_root`.
fn check_incremental(p: &Script, rs: &[ContextReceipt]) {
    assert_eq!(rs.len(), p.request_count(), "one receipt per request");
    for (i, r) in rs.iter().enumerate() {
        let all = non_system(&p.request(i).messages);
        let expected: Vec<(usize, String)> = if i == 0 {
            all.clone()
        } else {
            let before: HashSet<String> = non_system(&p.request(i - 1).messages)
                .into_iter()
                .map(|(_, h)| h)
                .collect();
            all.iter()
                .filter(|(_, h)| !before.contains(h))
                .cloned()
                .collect()
        };
        assert_eq!(listed(r), expected, "call {}: new or changed messages", i + 1);
        assert_eq!(
            field(r, "history_len"),
            json!(all.len()),
            "call {}: history_len",
            i + 1
        );
        let joined: Vec<&str> = all.iter().map(|(_, h)| h.as_str()).collect();
        assert_eq!(
            field(r, "history_root"),
            json!(sha256_hex(joined.join("\n").as_bytes())),
            "call {}: history_root commits to every hash in request order",
            i + 1
        );
    }
}

fn mem(id: &str, content: &str) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind: MemoryKind::Feedback,
        description: None,
        embedding: None,
    }
}

#[tokio::test]
async fn test_cr15_first_receipt_should_list_all_and_later_ones_only_new_messages() {
    // A recall block makes request positions differ from stored positions.
    let store = Arc::new(InMemoryStore::new());
    store.record(mem("m-a", "zorblax note")).await.unwrap();
    let mut r = rig(
        vec![vec![read("r1")], vec![read("r2")]],
        Opts {
            memory: Some(store),
            ..Default::default()
        },
    );

    let rs = receipts(&r.turn("zorblax please").await);

    assert_eq!(rs.len(), 3);
    check_incremental(&r.provider, &rs);
    let first_all = non_system(&r.provider.request(0).messages);
    assert_eq!(listed(&rs[0]), first_all, "the first receipt lists every message");
    for i in 1..3 {
        let all = non_system(&r.provider.request(i).messages).len();
        assert!(
            listed(&rs[i]).len() < all,
            "call {}: {} listed of {all}",
            i + 1,
            listed(&rs[i]).len()
        );
    }
    // The positions are request positions: the recall system message shifts them.
    assert_eq!(listed(&rs[1]).first().map(|(i, _)| *i), Some(3));
}

#[tokio::test]
async fn test_cr15_a_later_turn_should_list_only_what_the_turn_added() {
    let mut r = rig(vec![], Opts::default());

    let first = receipts(&r.turn("one").await);
    let second = receipts(&r.turn("two").await);

    let all: Vec<ContextReceipt> = first.into_iter().chain(second).collect();
    check_incremental(&r.provider, &all);
    // Turn 2 adds the assistant answer of turn 1 and the new user message.
    assert_eq!(listed(&all[1]).len(), 2);
}

#[tokio::test]
async fn test_cr15_history_sources_should_stay_proportional_to_what_each_call_adds() {
    // 30 tool rounds: a full listing would reach ~60 sources per receipt.
    let rounds: Vec<_> = (1..=30).map(|i| vec![read(&format!("t{i}"))]).collect();
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("keep reading").await);

    assert_eq!(rs.len(), 31);
    check_incremental(&r.provider, &rs);
    for i in 1..rs.len() {
        let added = non_system(&r.provider.request(i).messages).len()
            - non_system(&r.provider.request(i - 1).messages).len();
        let rewritten: usize = rs[i].compaction.iter().map(|s| s.replaced.len()).sum();
        let n = listed(&rs[i]).len();
        assert!(
            n <= added + rewritten,
            "call {}: {n} listed, {added} added, {rewritten} rewritten",
            i + 1
        );
        assert!(n <= 8, "call {}: {n} sources", i + 1);
    }
    assert_eq!(field(&rs[30], "history_len"), json!(61));
    // Size must not grow with the history. Compaction steps come and go with the
    // call (a rewrite is logged once), so measure the receipt without them.
    let size = |i: usize| {
        let mut r = rs[i].clone();
        r.compaction.clear();
        serde_json::to_string(&r).unwrap().len()
    };
    let slack = 100; // a few more digits in indexes and counts
    for i in 20..rs.len() {
        assert!(
            size(i) <= size(13) + slack,
            "call {}: {} bytes vs {} at call 14",
            i + 1,
            size(i),
            size(13)
        );
    }
}

#[tokio::test]
async fn test_cr15_a_compacted_message_should_be_listed_again_with_its_new_hash() {
    // Eight rounds: results older than five assistant turns are summarised.
    let rounds: Vec<_> = (1..=8).map(|i| vec![read(&format!("g{i}"))]).collect();
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("keep reading").await);

    check_incremental(&r.provider, &rs);
    let with_graduated: Vec<&ContextReceipt> = rs
        .iter()
        .filter(|x| x.compaction.iter().any(|s| s.strategy.name == "graduated"))
        .collect();
    assert!(!with_graduated.is_empty(), "scenario must compact");
    for receipt in with_graduated {
        let step = receipt
            .compaction
            .iter()
            .find(|s| s.strategy.name == "graduated")
            .unwrap();
        let Some(SourceRef::Message { sha256: new, .. }) = &step.derived else {
            panic!("graduated step has a derived message: {step:?}");
        };
        assert!(
            listed(receipt).iter().any(|(_, h)| h == new),
            "the rewritten message ({new}) is listed again"
        );
    }
}

#[tokio::test]
async fn test_cr15_reordered_but_unchanged_messages_should_not_be_listed_again() {
    // The test model is an "unknown" one, so middle-zone reordering is on: tool
    // results are ordered by relevance to the latest user message, which changes as
    // results are added. Unchanged messages that move are not relisted, while
    // `history_root` (order-sensitive) changes.
    let tags = ["omega", "zeta", "kappa", "sigma", "delta", "theta"];
    let rounds: Vec<_> = tags.iter().map(|t| vec![read(t)]).collect();
    let mut r = rig(rounds, Opts::default());

    let rs = receipts(&r.turn("theta delta zeta kappa").await);

    check_incremental(&r.provider, &rs);
    let order = |i: usize| -> Vec<String> {
        non_system(&r.provider.request(i).messages)
            .into_iter()
            .map(|(_, h)| h)
            .collect()
    };
    let mut moved = false;
    for i in 1..rs.len() {
        let (prev, cur) = (order(i - 1), order(i));
        let common_prev: Vec<&String> = prev.iter().filter(|h| cur.contains(h)).collect();
        let common_cur: Vec<&String> = cur.iter().filter(|h| prev.contains(h)).collect();
        if common_prev != common_cur {
            moved = true;
            assert_ne!(
                field(&rs[i], "history_root"),
                field(&rs[i - 1], "history_root")
            );
        }
    }
    assert!(moved, "harness: the scenario must reorder at least one unchanged message");
}
