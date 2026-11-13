#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G22 of `docs/plans/fts5-recall-spec.md` (review round 2): an encrypted store never gets an
//! FTS index. `open_without_fts` drops any FTS table, mapping table and triggers it finds (in
//! one `BEGIN IMMEDIATE`), and a plain `open` of a database holding the encryption sentinel
//! skips FTS setup, so `hone memory ...` subcommands and other plain opens don't reinstate it.
//! API: `SqliteStore::open_without_fts` (see `fts5_noindex.rs`).

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

async fn fts_objects(path: &Path) -> Vec<String> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();
    let names = sqlx::query("SELECT type || ' ' || name FROM sqlite_master WHERE name LIKE '%fts%' ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    pool.close().await;
    names
}

#[tokio::test]
async fn test_g22_a_plain_open_of_an_encrypted_database_should_not_install_fts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let inner = SqliteStore::open_without_fts(&path).await.unwrap();
        let store = EncryptedStore::new(inner, "pw").await.unwrap();
        store.record(mem("a", "staging database password rotation runbook", MemoryKind::Feedback, 1)).await.unwrap();
    }
    assert!(fts_objects(&path).await.is_empty(), "harness: the encrypted database starts without FTS");

    // `hone memory ls`, `forget` and friends open the file plainly.
    drop(SqliteStore::open(&path).await.unwrap());
    drop(SqliteStore::open_with_decay_interval(&path, 0).await.unwrap());

    assert_eq!(fts_objects(&path).await, Vec::<String>::new(), "no FTS table, map or triggers after a plain open");
}

#[tokio::test]
async fn test_g22_open_without_fts_should_drop_the_fts_objects_it_finds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let plain = SqliteStore::open_with_decay_interval(&path, 3_153_600_000).await.unwrap();
        plain.record(mem("a", "zorblax note about alpacas", MemoryKind::Feedback, 1)).await.unwrap();
    }
    assert!(!fts_objects(&path).await.is_empty(), "harness: a plain store has the FTS objects");

    let store = SqliteStore::open_without_fts(&path).await.unwrap();

    assert_eq!(fts_objects(&path).await, Vec::<String>::new(), "table, map, shadow tables and triggers are gone");
    // And it still works as a store: writes no longer touch an index, recall ranks in Rust.
    store.record(mem("b", "zorblax note about quokkas", MemoryKind::Feedback, 2)).await.unwrap();
    let got = store.recall(&q("zorblax quokkas", 5, None)).await.unwrap();
    assert_eq!(got.first().map(|m| m.id.as_str()), Some("b"));
    assert_eq!(fts_objects(&path).await, Vec::<String>::new());
}
