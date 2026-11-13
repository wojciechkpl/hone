#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]
//! CR27 of `docs/plans/context-receipts-spec.md` (review round 2 for PR C):
//! `append_context_receipts` must not leave a pooled connection inside a transaction when
//! its future is dropped part-way (cancelled). A leaked `BEGIN IMMEDIATE` keeps SQLite's
//! write lock, so the NEXT append (on another pooled connection) fails with "database is
//! locked". No new API; the store is linked with `#[path]` as in `context_show_render.rs`.
//!
//! Not tested here: the "deprecated `is_read_only_tool`" build warning (a `cargo build`
//! warning cannot be asserted cheaply from a test) and the docs wording.

#[path = "../src/session.rs"]
mod session;

use std::time::Duration;

use hone_proto::ContextReceipt;
use session::SessionStore;

fn receipt(turn: u32) -> ContextReceipt {
    ContextReceipt {
        turn,
        call: 1,
        model: "mock:test".to_owned(),
        request_sha256: "ab".repeat(32),
        blocks: vec![],
        dropped: vec![],
        compaction: vec![],
        tools_offered: vec![],
        history_len: 0,
        history_root: String::new(),
    }
}

fn batch(agent: &str, n: u64) -> hone_core::ReceiptBatch {
    hone_core::ReceiptBatch {
        agent_id: agent.to_owned(),
        items: (1..=n).map(|i| (i, receipt(i as u32))).collect(),
    }
}

#[tokio::test]
async fn test_cr27_a_dropped_append_should_not_leave_the_write_lock_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::open(&dir.path().join("sessions.db")).await.unwrap();
    store.save_session("s", &[]).await.unwrap();

    // Cancel the append after k polls, for many k, so some drop lands between BEGIN and COMMIT.
    for k in 1..60u32 {
        let agent = format!("cancelled-{k}");
        let cancelled = batch(&agent, 20);
        let mut fut = Box::pin(store.append_context_receipts("s", &cancelled));
        for _ in 0..k {
            if futures::poll!(fut.as_mut()).is_ready() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        drop(fut);

        // Another append must still get the write lock.
        let ok = tokio::time::timeout(
            Duration::from_secs(20),
            store.append_context_receipts("s", &batch(&format!("after-{k}"), 1)),
        )
        .await
        .unwrap_or_else(|_| panic!("append after a cancelled one (k = {k}) hung"));
        ok.unwrap_or_else(|e| panic!("append after a cancelled one (k = {k}) failed: {e:#}"));
    }
}
