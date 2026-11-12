#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G21 of `docs/plans/fts5-recall-spec.md` (review round 2): encrypted memories keep a
//! compatible format and compact vectors.
//!
//! Contract pinned:
//! - a new row's `content` stays `enc:v1:<base64>` (readable by older binaries);
//! - its embedding is stored encrypted in the `embedding` column as a BINARY blob whose first
//!   byte is the marker `0xE1`, followed by the 12-byte nonce and the AES-GCM ciphertext of the
//!   little-endian f32s (so about 1 + 12 + 4n + 16 bytes, at most a few bytes of padding): not
//!   JSON, not plaintext floats. A memory without an embedding has a NULL embedding column;
//! - an older reader (a plain `SqliteStore`) sees the ciphertext content text and does not
//!   fail on the blob;
//! - rows written as `enc:v2:` by earlier builds of this branch (fixture
//!   `tests/fixtures/enc_v2.db`, passphrase `fixture-pass`, rows `v2-row` with embedding
//!   [1.0, 0.0, 0.5, 0.25] and `v1-row` without) still decrypt;
//! - an `enc:v1` row whose embedding column holds PLAINTEXT floats (written by an older
//!   binary) is re-encrypted once, when an `EncryptedStore` is opened over the database.
//! No new API (`SqliteStore::open_without_fts` is pinned in `fts5_noindex.rs`).

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

const MARKER: u8 = 0xE1;

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
    let blob = embedding.expect("an embedding column value");
    assert_eq!(blob[0], MARKER, "the marker byte");
    assert!(blob[0] != b'[' && blob[0] != b'{', "not JSON");
    assert!(blob.len() >= 1 + 12 + 16 + 16, "marker + nonce + tag + ciphertext: {} bytes", blob.len());
    assert!(blob.len() <= 64, "compact: {} bytes for 4 floats", blob.len());
    for window in vector.chunks(1) {
        assert!(!contains(&blob, &f32_bytes(window)) || window[0] == 0.0, "no plaintext float {window:?} in the blob");
    }
    assert!(!contains(&blob, &f32_bytes(&vector)), "the plaintext vector is not in the blob");
    let (_, none) = raw(&path, "without").await;
    assert!(none.is_none() || none.as_ref().is_some_and(Vec::is_empty), "no embedding, no blob");

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
async fn test_g21_an_enc_v1_row_with_a_plaintext_embedding_should_be_re_encrypted_on_open_and_v2_rows_still_decrypt() {
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
    let blob = after.expect("the column still holds the vector");
    assert_eq!(blob[0], MARKER, "re-encrypted once, on open");
    assert!(!contains(&blob, &plain), "no plaintext floats remain");
    let got = store.recall(&q("pangolins", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]), "and it still decrypts");
    drop(store);
    // A second open leaves the blob alone (idempotent).
    let _ = open(&path, "pw").await;
    let (_, again) = raw(&path, "old").await;
    assert_eq!(again.as_ref().map(|b| b[0]), Some(MARKER));

    // Rows written as enc:v2 by earlier builds still decrypt (fixture from this branch's code).
    rows_written_as_enc_v2_by_earlier_builds_should_still_decrypt().await;
}
