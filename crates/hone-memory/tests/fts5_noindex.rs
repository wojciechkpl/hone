#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G15 (no FTS index under an encrypted store) and G17 (`recall_fts5` on a store without an
//! index returns real bm25 scores) of `docs/plans/fts5-recall-spec.md`, review round 1.
//!
//! API required (new): `SqliteStore::open_without_fts(path: &Path) -> Result<SqliteStore,
//! MemoryError>`: opens (creating if needed) the database WITHOUT the FTS5 table, mapping
//! table or triggers (and without creating them later); it is what the CLI uses as the inner
//! store of an `EncryptedStore`, whose content is ciphertext and cannot be indexed. On such a
//! store `recall_fts5` has no index to use and returns the results of the in-Rust BM25 with
//! its real scores (positive, descending, not all equal), as `recall` ranks them.

mod fts5_support;

use std::path::Path;

use fts5_support::{corpus, mem, q, queries};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

async fn schema_names(path: &Path) -> Vec<(String, String)> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();
    let rows = sqlx::query("SELECT type, name FROM sqlite_master ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get::<String, _>(0), r.get::<String, _>(1)))
        .collect();
    pool.close().await;
    rows
}

#[tokio::test]
async fn test_g15_an_encrypted_stores_inner_database_should_have_no_fts_table_or_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let inner = SqliteStore::open_without_fts(&path).await.unwrap();
        let store = EncryptedStore::new(inner, "correct horse").await.unwrap();
        store.record(mem("a", "staging database password rotation runbook", MemoryKind::Feedback, 1)).await.unwrap();
        let got = store.recall(&q("how do I rotate the staging database runbook", 5, None)).await.unwrap();
        assert_eq!(got.len(), 1, "ranked recall still works over the decrypted corpus");
    }

    let names = schema_names(&path).await;
    let fts: Vec<&(String, String)> = names.iter().filter(|(_, n)| n.contains("fts")).collect();
    assert!(fts.is_empty(), "no FTS objects in the inner database: {fts:?}");
    // And re-opening without FTS still creates none.
    drop(SqliteStore::open_without_fts(&path).await.unwrap());
    assert!(schema_names(&path).await.iter().all(|(_, n)| !n.contains("fts")));
}

#[tokio::test]
async fn test_g17_fts5_recall_on_a_store_without_an_index_should_return_real_bm25_scores() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_without_fts(&dir.path().join("memory.db")).await.unwrap();
    for m in corpus() {
        store.record(m).await.unwrap();
    }

    for (query, winner) in queries() {
        let scored = store.recall_fts5(&q(query, 5, None), &[]).await.unwrap();
        let ranked = store.recall(&q(query, 5, None)).await.unwrap();

        assert!(!scored.is_empty(), "{query:?}");
        assert_eq!(scored[0].memory.id, winner, "{query:?}");
        assert_eq!(
            scored.iter().map(|s| s.memory.id.as_str()).collect::<Vec<_>>(),
            ranked.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            "{query:?}: same order as the BM25 recall"
        );
        assert!(scored.iter().all(|s| s.score > 0.0), "{query:?}");
        assert!(scored.windows(2).all(|w| w[0].score >= w[1].score), "{query:?}: descending");
        if scored.len() > 1 {
            assert!(scored[0].score > scored[scored.len() - 1].score, "{query:?}: real scores, not a constant");
        }
    }
}
