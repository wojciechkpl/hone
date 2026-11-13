#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G38 of `docs/plans/fts5-recall-spec.md` (review round 5): `EncryptedStore::new` reports what
//! it did, so the CLI can print it to stderr whatever `RUST_LOG` says.
//!
//! INTERFACE ASSUMPTION (the spec names no type; minimal shape chosen by the test author):
//! `EncryptedStore::open_report(&self) -> &hone_memory::OpenReport`, where
//! `OpenReport { pub passphrase_pinned: bool, pub vectors_migrated: usize,
//! pub vectors_skipped: usize }` (`Debug`). `passphrase_pinned` is true only when there was
//! nothing to check the passphrase against and it was pinned as the first one. The CLI side
//! (printing to stderr) is not tested here.

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};

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

fn vectors() -> Vec<(&'static str, &'static str, Option<Vec<f32>>)> {
    vec![
        ("a", "legacy note about alpha", Some(vec![0.5, 0.5, 0.25, 0.125])),
        ("b", "legacy note about beta", Some(vec![1.0, 0.0, 0.5, 0.25])),
        ("c", "legacy note about gamma", None),
    ]
}

#[tokio::test]
async fn test_g38_opening_a_sentinel_only_database_should_report_the_pin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");

    let store = open(&path, "first").await.unwrap();

    let report = store.open_report();
    assert!(report.passphrase_pinned, "{report:?}");
    assert_eq!(report.vectors_migrated, 0, "{report:?}");
    assert_eq!(report.vectors_skipped, 0, "{report:?}");
}

#[tokio::test]
async fn test_g38_reopening_a_database_with_a_key_check_should_report_nothing_to_say() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let store = open(&path, "pw").await.unwrap();
        store.record(mem("a", "something", MemoryKind::Feedback, 1)).await.unwrap();
    }

    let store = open(&path, "pw").await.unwrap();

    let report = store.open_report();
    assert!(!report.passphrase_pinned, "{report:?}");
    assert_eq!((report.vectors_migrated, report.vectors_skipped), (0, 0), "{report:?}");
}

#[tokio::test]
async fn test_g38_a_migration_should_report_how_many_vectors_moved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "pw", &vectors()).await;

    let store = open(&path, "pw").await.unwrap();

    let report = store.open_report();
    assert_eq!(report.vectors_migrated, 2, "{report:?}");
    assert_eq!(report.vectors_skipped, 0, "{report:?}");
    assert!(!report.passphrase_pinned, "encrypted rows existed to check against: {report:?}");
    assert_eq!(store.recall(&q("alpha", 3, None)).await.unwrap()[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]));
}

#[tokio::test]
async fn test_g38_an_undecryptable_blob_should_be_reported_as_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "pw", &vectors()).await;
    let mut bad = vec![0xE1, 0x48, 0x4F, 0x4E, 0x45, 0x56, 0x31, 0x00];
    bad.extend((0..44u8).map(|i| i.wrapping_mul(7).wrapping_add(3)));
    let p = pool(&path).await;
    sqlx::query("UPDATE memories SET embedding = ?1 WHERE id = 'b'").bind(&bad).execute(&p).await.unwrap();
    p.close().await;

    let store = open(&path, "pw").await.unwrap();

    let report = store.open_report();
    assert_eq!(report.vectors_migrated, 1, "{report:?}");
    assert_eq!(report.vectors_skipped, 1, "{report:?}");
}
