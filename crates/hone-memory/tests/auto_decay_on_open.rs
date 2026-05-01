//! Red-phase tests for R11: `SqliteStore::open` must call `decay(0.95, now)`
//! once after schema migration succeeds.
//!
//! These tests MUST FAIL on `main` because `SqliteStore::open` does not yet
//! call `decay` automatically. The assertion in `open_decays_existing_weights`
//! will fail at runtime (weight == 1.0, expected 0.95); the others may pass or
//! fail depending on the assertion, but none require a compile error.
//!
//! # Drop discipline
//!
//! Each test that writes records, then re-opens, explicitly drops the first
//! `SqliteStore` before calling `open` again.  This is necessary because:
//! 1. SQLite WAL-mode allows multiple readers but serialises writes.
//! 2. `SqlitePool::close()` is called when the pool is dropped, ensuring the
//!    WAL is flushed and the file is in a consistent state for the second open.
//!
//! Never call `decay` assertions on the *same* instance that inserted the
//! records — the auto-decay only runs inside `open()`.

use std::time::{SystemTime, UNIX_EPOCH};

use hone_memory::{Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time before epoch")
        .as_secs()
}

fn make_memory(id: &str, content: &str, weight: f64, decay_at: Option<u64>) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight,
        created_at: now_secs(),
        decay_at,
        kind: MemoryKind::Conversation,
        description: None,
    }
}

// ---------------------------------------------------------------------------
// Test 12 — open on a fresh path returns Ok, store has 0 memories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_empty_db_succeeds() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    let store = SqliteStore::open(&db_path)
        .await
        .expect("open on a fresh path must succeed");

    let all = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall on empty store must not error");

    assert_eq!(
        all.len(),
        0,
        "fresh store must have 0 memories; got {}",
        all.len()
    );
}

// ---------------------------------------------------------------------------
// Test 13 — open decays weights from 1.0 to 0.95
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_decays_existing_weights() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    // First open: insert a memory with weight 1.0.
    {
        let mut store = SqliteStore::open(&db_path)
            .await
            .expect("first open must succeed");
        store
            .record(make_memory("w1", "weight test content", 1.0, None))
            .await
            .expect("record");
        // Store is dropped here — pool is closed before second open.
    }

    // Second open: force interval=0 so decay always runs regardless of elapsed time.
    let store = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("second open must succeed");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall after second open");

    assert_eq!(results.len(), 1, "memory should still exist after decay");
    let weight = results[0].weight;
    assert!(
        (weight - 0.95).abs() < 1e-9,
        "weight after one open should be 0.95 (1.0 * 0.95); got {weight}"
    );
}

// ---------------------------------------------------------------------------
// Test 14 — two consecutive opens chain decay: 1.0 → 0.95 → ≈0.9025
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_chains_decay_across_opens() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    // First open: insert.
    {
        let mut store = SqliteStore::open(&db_path)
            .await
            .expect("first open");
        store
            .record(make_memory("chain1", "chaining decay content", 1.0, None))
            .await
            .expect("record");
    }

    // Second open: force interval=0 so decay always runs; weight becomes 0.95.
    {
        let _store = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("second open");
        // Drop immediately; we just want the open-time decay to run.
    }

    // Third open: force interval=0 again; weight becomes 0.95 * 0.95 = 0.9025.
    let store = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("third open");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall after third open");

    assert_eq!(results.len(), 1, "memory should still exist after two decays");
    let weight = results[0].weight;
    let expected = 0.95 * 0.95;
    assert!(
        (weight - expected).abs() < 1e-6,
        "weight after two opens should be ≈{expected:.6}; got {weight}"
    );
}

// ---------------------------------------------------------------------------
// Test 15 — open removes a memory whose decay_at is in the past
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_removes_memories_past_decay_at() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    let past = now_secs().saturating_sub(60); // 60 seconds ago

    // First open: insert a memory that is already expired.
    {
        let mut store = SqliteStore::open(&db_path)
            .await
            .expect("first open");
        store
            .record(make_memory("expired1", "already expired", 1.0, Some(past)))
            .await
            .expect("record expired memory");
    }

    // Second open: force interval=0 so decay always runs; decay_at is in the
    // past, so the memory should be deleted by `decay(0.95, now)`.
    let store = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("second open");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall after second open");

    assert_eq!(
        results.len(),
        0,
        "memory with decay_at in the past must be removed on open; got {} records",
        results.len()
    );
}

// ---------------------------------------------------------------------------
// Test 16 — open keeps a memory whose decay_at is in the future
// ---------------------------------------------------------------------------


#[tokio::test]
async fn open_keeps_memories_with_future_decay_at() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    let future = now_secs() + 3_600; // 1 hour from now

    // First open: insert a memory that expires in the future.
    {
        let mut store = SqliteStore::open(&db_path)
            .await
            .expect("first open");
        store
            .record(make_memory("future1", "future decay content", 1.0, Some(future)))
            .await
            .expect("record future memory");
    }

    // Second open: force interval=0 so decay always runs; decay_at > now, so
    // the memory survives with weight 0.95.
    let store = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("second open");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall after second open");

    assert_eq!(
        results.len(),
        1,
        "memory with future decay_at must survive the open-time decay; got {} records",
        results.len()
    );
    let weight = results[0].weight;
    assert!(
        (weight - 0.95).abs() < 1e-9,
        "surviving memory's weight must have decayed to 0.95; got {weight}"
    );
}

// ---------------------------------------------------------------------------
// Test 17 — two opens within 24h do NOT chain-decay (time-gate)
// ---------------------------------------------------------------------------
//
// Setup: use open_with_decay_interval(0) to force a first decay pass (giving
// the memory weight 0.95) and stamp last_decay_at = now.  Then immediately
// open again with the default 86400-second interval.  Because now - last_decay_at
// < 86400, the second open must skip decay and the weight stays at 0.95.

#[tokio::test]
async fn open_does_not_decay_within_24h() {
    let dir = TempDir::new().expect("create tempdir");
    let db_path = dir.path().join("memory.db");

    // First open (interval=0): insert a memory.  Decay fires on empty DB,
    // stamps last_decay_at.
    {
        let mut store = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("first open");
        store
            .record(make_memory("gate1", "time-gate test content", 1.0, None))
            .await
            .expect("record");
    }

    // Second open (interval=0): decay fires, weight becomes 0.95, stamps
    // last_decay_at = now.
    {
        let _store = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("second open (force decay)");
    }

    // Third open (default 86400s interval): last_decay_at was just set
    // seconds ago, so now - last_decay_at < 86400.  Decay must be skipped.
    let store = SqliteStore::open(&db_path)
        .await
        .expect("third open within 24h");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall after third open");

    assert_eq!(results.len(), 1, "memory must still exist; got {}", results.len());
    let weight = results[0].weight;
    assert!(
        (weight - 0.95).abs() < 1e-9,
        "weight must remain at 0.95 (time-gate skipped second decay); got {weight}"
    );
}
