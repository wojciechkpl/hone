#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G37 of `docs/plans/fts5-recall-spec.md` (review round 5): a sealed vector belongs to its
//! row. Vectors are sealed with AES-GCM associated data (the memory id and the row's content
//! ciphertext), so a sealed vector whose row was replaced outside `EncryptedStore` (a plain
//! store, an older binary; `INSERT OR REPLACE` does not fire the AFTER DELETE trigger because
//! SQLite's recursive triggers are off) reads as NO embedding, and a blob moved to another id
//! reads as none too. Only observable behaviour is pinned (reads through the store).

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

async fn two_rows(path: &Path) {
    let store = open(path, "pw").await.unwrap();
    for (id, content, v) in [("x", "note about xylophones", [1.0f32, 0.0, 0.0, 0.5]), ("y", "note about yaks", [0.0, 1.0, 0.5, 0.0])] {
        let mut m = mem(id, content, MemoryKind::Feedback, 5);
        m.embedding = Some(v.to_vec());
        store.record(m).await.unwrap();
    }
}

async fn exec(path: &Path, sql: &str) {
    let p = pool(path).await;
    sqlx::query(sql).execute(&p).await.unwrap();
    p.close().await;
}

#[tokio::test]
async fn test_g37_a_row_replaced_by_a_plain_store_should_read_as_no_embedding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    two_rows(&path).await;
    {
        // A plain run replaces x (INSERT OR REPLACE): new content, no vector.
        let plain = SqliteStore::open_without_fts(&path).await.unwrap();
        plain.record(mem("x", "replacement written by a plain store", MemoryKind::Feedback, 6)).await.unwrap();
    }
    let store = open(&path, "pw").await.unwrap();

    let got = store.recall_by_id("x").await.unwrap().expect("the replacement is there");

    assert_eq!(got.content, "replacement written by a plain store");
    assert!(got.embedding.is_none(), "the OLD vector must not survive the replacement: {:?}", got.embedding);
    let listed = store.recall(&q("replacement", 5, None)).await.unwrap();
    assert!(listed[0].embedding.is_none(), "{:?}", listed[0].embedding);
    assert_eq!(store.recall_by_id("y").await.unwrap().unwrap().embedding.as_deref(), Some(&[0.0f32, 1.0, 0.5, 0.0][..]), "other rows keep theirs");
}

#[tokio::test]
async fn test_g37_a_row_replaced_by_other_valid_ciphertext_should_not_keep_the_old_vector() {
    // An older binary rewrites x with a perfectly valid ciphertext (here: y's), no vector.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    two_rows(&path).await;
    exec(&path, "INSERT OR REPLACE INTO memories (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
                 SELECT 'x', content, tags, weight, created_at, decay_at, kind, NULL, NULL FROM memories WHERE id = 'y'").await;
    let store = open(&path, "pw").await.unwrap();

    let got = store.recall_by_id("x").await.unwrap().unwrap();

    assert_eq!(got.content, "note about yaks", "harness: x now holds y's content");
    assert!(got.embedding.is_none(), "x's old sealed vector belonged to its old content: {:?}", got.embedding);
}

#[tokio::test]
async fn test_g37_a_sealed_blob_copied_to_another_id_should_read_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    two_rows(&path).await;
    // y gets x's blob (and loses its own).
    exec(&path, "INSERT OR REPLACE INTO memory_vectors_sealed (id, blob) SELECT 'y', blob FROM memory_vectors_sealed WHERE id = 'x'").await;
    let store = open(&path, "pw").await.unwrap();

    let y = store.recall_by_id("y").await.unwrap().unwrap();

    assert!(y.embedding.is_none(), "x's vector must not be read as y's: {:?}", y.embedding);
    let x = store.recall_by_id("x").await.unwrap().unwrap();
    assert_eq!(x.embedding.as_deref(), Some(&[1.0f32, 0.0, 0.0, 0.5][..]), "x keeps its own");
}

#[tokio::test]
async fn test_g37_swapped_blobs_should_both_read_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    two_rows(&path).await;
    exec(&path, "UPDATE memory_vectors_sealed SET id = 'tmp' WHERE id = 'x'").await;
    exec(&path, "UPDATE memory_vectors_sealed SET id = 'x' WHERE id = 'y'").await;
    exec(&path, "UPDATE memory_vectors_sealed SET id = 'y' WHERE id = 'tmp'").await;
    let store = open(&path, "pw").await.unwrap();

    assert!(store.recall_by_id("x").await.unwrap().unwrap().embedding.is_none());
    assert!(store.recall_by_id("y").await.unwrap().unwrap().embedding.is_none());
}

#[tokio::test]
async fn test_g37_a_vector_recorded_through_the_store_should_read_back_and_follow_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = open(&path, "pw").await.unwrap();
    let mut m = mem("x", "first content", MemoryKind::Feedback, 1);
    m.embedding = Some(vec![1.0, 2.0, 3.0, 4.0]);
    store.record(m.clone()).await.unwrap();
    assert_eq!(store.recall_by_id("x").await.unwrap().unwrap().embedding.as_deref(), Some(&[1.0f32, 2.0, 3.0, 4.0][..]));

    m.content = "second content".to_owned();
    m.embedding = Some(vec![4.0, 3.0, 2.0, 1.0]);
    store.record(m).await.unwrap();

    let got = store.recall_by_id("x").await.unwrap().unwrap();
    assert_eq!(got.content, "second content");
    assert_eq!(got.embedding.as_deref(), Some(&[4.0f32, 3.0, 2.0, 1.0][..]), "bound to the NEW content, still readable");
    drop(store);
    let reopened = open(&path, "pw").await.unwrap();
    assert_eq!(reopened.recall_by_id("x").await.unwrap().unwrap().embedding.as_deref(), Some(&[4.0f32, 3.0, 2.0, 1.0][..]));
}
