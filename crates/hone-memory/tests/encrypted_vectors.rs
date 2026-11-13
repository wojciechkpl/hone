#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G21 of `docs/plans/fts5-recall-spec.md` (review round 2): encrypted memories keep a
//! compatible format and compact vectors.
//!
//! Contract pinned:
//! - a new row's `content` stays `enc:v1:<base64>` (readable by older binaries);
//! - its embedding is stored encrypted in the side table `memory_vectors_sealed(id TEXT PRIMARY
//!   KEY, blob BLOB NOT NULL)` (G29, review round 4; no longer in `memories.embedding`): the
//!   12-byte nonce and the AES-GCM ciphertext of the little-endian f32s (12 + 4n + 16 bytes),
//!   not JSON, not plaintext floats. `memories.embedding` stays NULL for encrypted rows, and a
//!   memory without an embedding has no side-table row;
//! - an older reader (a plain `SqliteStore`) sees the ciphertext content text and NO embedding;
//! - rows written as `enc:v2:` by earlier builds of this branch (fixture
//!   `tests/fixtures/enc_v2.db`, passphrase `fixture-pass`, rows `v2-row` with embedding
//!   [1.0, 0.0, 0.5, 0.25] and `v1-row` without) still decrypt;
//! - an `enc:v1` row whose embedding column holds PLAINTEXT floats (written by an older
//!   binary) is migrated once, when an `EncryptedStore` is opened over the database: sealed into
//!   the side table, the column set to NULL.
//! No new API (`SqliteStore::open_without_fts` is pinned in `fts5_noindex.rs`).

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;


async fn open(path: &Path, pass: &str) -> EncryptedStore<SqliteStore> {
    let inner = SqliteStore::open_without_fts(path).await.unwrap();
    EncryptedStore::new(inner, pass).await.unwrap()
}

async fn raw(path: &Path, id: &str) -> (String, Option<Vec<u8>>) {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();
    let row = sqlx::query("SELECT content, embedding FROM memories WHERE id = ?1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let out = (row.get::<String, _>(0), row.get::<Option<Vec<u8>>, _>(1));
    pool.close().await;
    out
}

async fn sealed(path: &Path, id: &str) -> Option<Vec<u8>> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();
    let row = sqlx::query("SELECT blob FROM memory_vectors_sealed WHERE id = ?1").bind(id).fetch_optional(&pool).await;
    pool.close().await;
    match row {
        Ok(r) => r.map(|r| r.get::<Vec<u8>, _>(0)),
        Err(_) => None, // no side table yet
    }
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
async fn test_g21_new_rows_should_keep_enc_v1_content_and_store_the_vector_as_a_compact_encrypted_blob() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let vector = vec![1.5f32, 0.25, 0.125, 0.0625];
    {
        let store = open(&path, "pw").await;
        let mut with = mem("with", "note about quokkas and alpacas", MemoryKind::Feedback, 1);
        with.embedding = Some(vector.clone());
        store.record(with).await.unwrap();
        store.record(mem("without", "note about narwhals", MemoryKind::Feedback, 2)).await.unwrap();
    }

    let (content, embedding) = raw(&path, "with").await;
    assert!(content.starts_with("enc:v1:"), "content stays enc:v1: {}", &content[..content.len().min(12)]);
    assert!(embedding.is_none(), "memories.embedding stays NULL for encrypted rows");
    let blob = sealed(&path, "with").await.expect("a row in memory_vectors_sealed");
    assert!(blob.len() >= 12 + 16 + 16, "nonce + tag + ciphertext: {} bytes", blob.len());
    assert!(blob.len() <= 64, "compact: {} bytes for 4 floats", blob.len());
    assert!(blob[0] != b'[' && blob[0] != b'{', "not JSON");
    assert!(!contains(&blob, &f32_bytes(&vector)), "the plaintext vector is not in the blob");
    let (_, none) = raw(&path, "without").await;
    assert!(none.is_none(), "no embedding, no column value");
    assert!(sealed(&path, "without").await.is_none(), "and no sealed row");

    // Round trip, semantic recall and an older (plain) reader.
    let store = open(&path, "pw").await;
    let got = store.recall(&q("quokkas", 5, None)).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].content, "note about quokkas and alpacas");
    assert_eq!(got[0].embedding.as_deref(), Some(&vector[..]), "the returned memory carries its decrypted embedding");
    let scored = store.recall_semantic(&[1.5, 0.25, 0.125, 0.0625], 3).await.unwrap();
    assert_eq!(scored.first().map(|s| s.memory.id.as_str()), Some("with"), "semantic recall still works");
    assert_eq!(scored[0].memory.content, "note about quokkas and alpacas");
    drop(store);
    let plain = SqliteStore::open_without_fts(&path).await.unwrap();
    let all = plain.recall(&q("", 10, None)).await.unwrap();
    let seen = all.iter().find(|m| m.id == "with").expect("an older reader still lists the row");
    assert!(seen.content.starts_with("enc:v1:"), "it sees the ciphertext content text");
    assert!(seen.embedding.is_none(), "and no embedding at all: no bogus vector for an older reader");
}

async fn rows_written_as_enc_v2_by_earlier_builds_should_still_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/enc_v2.db"), &path).unwrap();
    let (v2_content, _) = raw(&path, "v2-row").await;
    assert!(v2_content.starts_with("enc:v2:"), "harness: the fixture holds an enc:v2 row");

    let store = open(&path, "fixture-pass").await;

    let got = store.recall(&q("quokkas", 5, None)).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "v2-row");
    assert_eq!(got[0].content, "fixture note about quokkas");
    assert_eq!(got[0].embedding.as_deref(), Some(&[1.0f32, 0.0, 0.5, 0.25][..]));
    let v1 = store.recall(&q("alpacas", 5, None)).await.unwrap();
    assert_eq!(v1[0].content, "fixture plain note about alpacas");
    let scored = store.recall_semantic(&[1.0, 0.0, 0.5, 0.25], 2).await.unwrap();
    assert_eq!(scored[0].memory.id, "v2-row", "semantic recall over a v2 row");
}

#[tokio::test]
async fn test_g21_an_enc_v1_row_with_a_plaintext_embedding_should_be_migrated_on_open_and_v2_rows_still_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let store = open(&path, "pw").await;
        store.record(mem("old", "legacy note about pangolins", MemoryKind::Feedback, 1)).await.unwrap();
    }
    // What an older binary wrote: enc:v1 content and the plaintext vector in the column.
    let plain = f32_bytes(&[0.5, 0.5, 0.25, 0.125]);
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap();
        sqlx::query("UPDATE memories SET embedding = ?1 WHERE id = 'old'").bind(&plain).execute(&pool).await.unwrap();
        pool.close().await;
    }
    let (_, before) = raw(&path, "old").await;
    assert_eq!(before.as_deref(), Some(&plain[..]), "harness: the plaintext vector is in the column");

    let store = open(&path, "pw").await;

    let (_, after) = raw(&path, "old").await;
    assert!(after.is_none(), "the plaintext column value is gone (set to NULL)");
    let blob = sealed(&path, "old").await.expect("sealed into the side table");
    assert!(!contains(&blob, &plain), "no plaintext floats in the sealed blob");
    let got = store.recall(&q("pangolins", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]), "and it still decrypts");
    drop(store);
    // A second open leaves everything alone (idempotent).
    let again_blob = sealed(&path, "old").await;
    let _ = open(&path, "pw").await;
    assert_eq!(sealed(&path, "old").await, again_blob);
    assert!(raw(&path, "old").await.1.is_none());

    // Rows written as enc:v2 by earlier builds still decrypt (fixture from this branch's code).
    rows_written_as_enc_v2_by_earlier_builds_should_still_decrypt().await;
}
