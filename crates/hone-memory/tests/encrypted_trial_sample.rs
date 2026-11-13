#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G35 of `docs/plans/fts5-recall-spec.md` (review round 5): a passphrase is verified against
//! encrypted rows chosen by a DIRECT query, not against the newest 1,000 rows of a listing.
//! Scenario (reviewer repro): a `main`-format database (sentinel description NULL) with 3
//! `enc:v1` rows holding plaintext vectors, plus >= 1,000 NEWER plaintext rows (plain runs on
//! the same database, which ES7 documents). Today the listing holds only plaintext rows, so a
//! WRONG passphrase finds "no encrypted rows", is pinned, seals the vectors under the wrong
//! key, and the right passphrase is then locked out.

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

const SENTINEL: &str = "__encrypted_store_salt__";

async fn open(path: &Path, pass: &str) -> Result<EncryptedStore<SqliteStore>, hone_memory::MemoryError> {
    let inner = SqliteStore::open_without_fts(path).await.unwrap();
    EncryptedStore::new(inner, pass).await
}

async fn pool(path: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap()
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// A `main`-style database: sentinel without key check, `enc:v1` rows, plaintext vectors.
async fn main_style_db(path: &Path, pass: &str, rows: &[(&str, &str, Option<Vec<f32>>)]) {
    {
        let store = open(path, pass).await.unwrap_or_else(|e| panic!("{e}"));
        for (id, content, _) in rows {
            let mut m = mem(id, content, MemoryKind::Feedback, 1);
            m.description = Some(format!("description of {id}"));
            m.weight = 0.7;
            store.record(m).await.unwrap();
        }
    }
    let p = pool(path).await;
    sqlx::query("UPDATE memories SET description = NULL WHERE id = ?1").bind(SENTINEL).execute(&p).await.unwrap();
    sqlx::query("DELETE FROM memory_vectors_sealed").execute(&p).await.ok();
    for (id, _, v) in rows {
        if let Some(v) = v {
            sqlx::query("UPDATE memories SET embedding = ?1 WHERE id = ?2").bind(f32_bytes(v)).bind(id).execute(&p).await.unwrap();
        }
    }
    p.close().await;
}

/// Everything a failed open must not change: every memories column, side table and meta.
async fn full_snapshot(path: &Path) -> Vec<String> {
    let p = pool(path).await;
    let mut out = Vec::new();
    for (name, sql) in [
        ("memories", "SELECT id, content, description, weight, created_at, kind, tags, hex(embedding) FROM memories ORDER BY id"),
        ("sealed", "SELECT id, hex(blob) FROM memory_vectors_sealed ORDER BY id"),
        ("meta", "SELECT * FROM memory_meta ORDER BY 1"),
    ] {
        if let Ok(rows) = sqlx::query(sql).fetch_all(&p).await {
            for r in rows {
                let cols: Vec<String> = (0..r.len())
                    .map(|i| {
                        r.try_get::<String, _>(i)
                            .or_else(|_| r.try_get::<i64, _>(i).map(|n| n.to_string()))
                            .or_else(|_| r.try_get::<f64, _>(i).map(|n| n.to_string()))
                            .unwrap_or_else(|_| "NULL".to_owned())
                    })
                    .collect();
                out.push(format!("{name}: {}", cols.join(" | ")));
            }
        }
    }
    p.close().await;
    out
}

/// `n` plaintext rows NEWER than everything else, in one transaction (what plain runs write).
async fn add_newer_plaintext_rows(path: &Path, n: u64) {
    let p = pool(path).await;
    let mut tx = p.begin().await.unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
             VALUES (?1, ?2, '[]', 1.0, ?3, NULL, 'conversation', NULL, NULL)",
        )
        .bind(format!("plain-{i:05}"))
        .bind(format!("plaintext row {i}"))
        .bind(2_000_000_000i64 + i as i64)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    p.close().await;
}

fn vectors() -> Vec<(&'static str, &'static str, Option<Vec<f32>>)> {
    vec![
        ("a", "legacy note about alpha", Some(vec![0.5, 0.5, 0.25, 0.125])),
        ("b", "legacy note about beta", Some(vec![1.0, 0.0, 0.5, 0.25])),
        ("c", "legacy note about gamma", Some(vec![0.0, 1.0, 0.0, 1.0])),
    ]
}

async fn sentinel_description(path: &Path) -> Option<String> {
    let p = pool(path).await;
    let d = sqlx::query("SELECT description FROM memories WHERE id = ?1").bind(SENTINEL).fetch_one(&p).await.unwrap().get(0);
    p.close().await;
    d
}

#[tokio::test]
async fn test_g35_a_wrong_passphrase_should_be_refused_however_many_newer_plaintext_rows_exist() {
    for newer in [1_000u64, 2_500] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        main_style_db(&path, "right", &vectors()).await;
        add_newer_plaintext_rows(&path, newer).await;
        assert!(sentinel_description(&path).await.is_none(), "harness: main format, no key check");
        let before = full_snapshot(&path).await;

        let wrong = open(&path, "WRONG").await;

        assert!(wrong.is_err(), "{newer} newer plaintext rows: a wrong passphrase must not be accepted and pinned");
        drop(wrong);
        assert_eq!(full_snapshot(&path).await, before, "{newer}: every column, the side table, meta and the sentinel are unchanged");
    }
}

#[tokio::test]
async fn test_g35_the_right_passphrase_should_then_open_with_vectors_intact_and_write_the_key_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "right", &vectors()).await;
    add_newer_plaintext_rows(&path, 1_000).await;
    drop(open(&path, "WRONG").await);

    let store = open(&path, "right").await.unwrap_or_else(|e| panic!("the right passphrase is not locked out: {e}"));

    let got = store.recall(&q("alpha", 5, None)).await.unwrap();
    assert_eq!(got[0].id, "a");
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]), "vectors intact");
    let b = store.recall_by_id("b").await.unwrap().unwrap();
    assert_eq!(b.embedding.as_deref(), Some(&[1.0f32, 0.0, 0.5, 0.25][..]));
    drop(store);
    assert!(sentinel_description(&path).await.is_some_and(|d| !d.is_empty()), "the key check was written");
    assert!(open(&path, "WRONG").await.is_err(), "and it now rejects a wrong passphrase");
}

#[tokio::test]
async fn test_g35_a_database_with_only_plaintext_rows_should_still_pin_the_first_passphrase() {
    // No encrypted row exists anywhere: nothing to check against, so the first passphrase is pinned.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "first", &[]).await;
    add_newer_plaintext_rows(&path, 1_200).await;

    assert!(open(&path, "first").await.is_ok());

    assert!(open(&path, "second").await.is_err(), "pinned");
    assert!(open(&path, "first").await.is_ok());
}
