#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G25, G26 and G27 of `docs/plans/fts5-recall-spec.md` (review round 3).
//!
//! Contract pinned:
//! - G25: `EncryptedStore::new` with a WRONG passphrase returns `Err` and changes nothing in the
//!   database (no row, no embedding column byte), even when legacy rows hold plaintext vectors
//!   that the migration would otherwise re-encrypt (under the wrong key: permanent loss). The
//!   passphrase is verified first: by a key-check value stored with the salt sentinel (written
//!   on first use or created from a successful trial decryption of an existing `enc:v1` row),
//!   or by trial-decrypting an `enc:v1` row. So a FRESH database (sentinel, no memories) also
//!   rejects a wrong passphrase on reopening.
//! - G26: a sealed embedding blob starts with the 8-byte magic `E1 48 4F 4E 45 56 31 00`
//!   (`\xE1HONEV1\0`). The migration decides by magic plus a successful trial decryption, so a
//!   legacy plaintext vector whose first byte happens to be `0xE1` is still re-encrypted. Blobs
//!   from earlier builds of this branch (1-byte marker, fixture `tests/fixtures/enc_marker1.db`,
//!   passphrase `fixture-pass`) still decrypt.
//! - G27 (what is testable here): a plain `SqliteStore` (today's decoder, which reads any blob
//!   as little-endian f32s) does not read a sealed row as a vector of the original dimension.
//!   `main`'s decoder cannot be run from this tree, so rejection by an older binary is not
//!   tested; the spec's fallback (CHANGELOG wording) is documentation.
//! Legacy rows are built the way `main` writes them: `enc:v1` content and the vector as plaintext
//! little-endian f32 bytes in the `embedding` column.

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

const MAGIC: [u8; 8] = [0xE1, 0x48, 0x4F, 0x4E, 0x45, 0x56, 0x31, 0x00];

async fn open(path: &Path, pass: &str) -> Result<EncryptedStore<SqliteStore>, hone_memory::MemoryError> {
    let inner = SqliteStore::open_without_fts(path).await.unwrap();
    EncryptedStore::new(inner, pass).await
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

async fn pool(path: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display())).await.unwrap()
}

/// Every row as (id, content, embedding bytes), ordered by id.
async fn snapshot(path: &Path) -> Vec<(String, String, Option<Vec<u8>>)> {
    let p = pool(path).await;
    let rows = sqlx::query("SELECT id, content, embedding FROM memories ORDER BY id")
        .fetch_all(&p)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get::<String, _>(0), r.get::<String, _>(1), r.get::<Option<Vec<u8>>, _>(2)))
        .collect();
    p.close().await;
    rows
}

async fn embedding_of(path: &Path, id: &str) -> Option<Vec<u8>> {
    snapshot(path).await.into_iter().find(|r| r.0 == id).unwrap().2
}

/// A vector whose first byte (little-endian) is `0xE1`.
fn e1_vector() -> Vec<f32> {
    let mut v = vec![f32::from_le_bytes([0xE1, 0x01, 0x80, 0x3F])];
    v.extend([0.25, -0.5, 0.75, 1.25, 2.5, -1.5, 0.125]);
    assert_eq!(f32_bytes(&v)[0], 0xE1);
    v
}

/// A database as `main` leaves it: sentinel + `enc:v1` rows whose vectors are plaintext.
async fn legacy_db(path: &Path, pass: &str, vectors: &[(&str, &str, Vec<f32>)]) {
    {
        let store = open(path, pass).await.unwrap_or_else(|e| panic!("{e}"));
        for (id, content, _) in vectors {
            store.record(mem(id, content, MemoryKind::Feedback, 1)).await.unwrap();
        }
    }
    let p = pool(path).await;
    for (id, _, v) in vectors {
        sqlx::query("UPDATE memories SET embedding = ?1 WHERE id = ?2").bind(f32_bytes(v)).bind(id).execute(&p).await.unwrap();
    }
    p.close().await;
}

fn legacy_vectors() -> Vec<(&'static str, &'static str, Vec<f32>)> {
    vec![
        ("a", "legacy note about alpha", vec![0.5, 0.5, 0.25, 0.125]),
        ("b", "legacy note about beta", vec![1.0, 0.0, 0.5, 0.25, 2.0, 3.0, 4.0, 5.0]),
        ("e1", "legacy note about gamma", e1_vector()),
    ]
}

// ---- G25 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g25_a_wrong_passphrase_should_fail_and_change_nothing_then_the_right_one_should_find_every_vector() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let vectors = legacy_vectors();
    legacy_db(&path, "right", &vectors).await;
    let before = snapshot(&path).await;
    assert_eq!(before.iter().filter(|r| r.2.is_some()).count(), 3, "harness: three plaintext vectors");

    let wrong = open(&path, "wrong").await;

    assert!(wrong.is_err(), "a wrong passphrase must be an error, not a silent migration under the wrong key");
    drop(wrong);
    assert_eq!(snapshot(&path).await, before, "no row, no embedding byte changed");

    // The right passphrase afterwards: every vector is intact.
    let store = open(&path, "right").await.unwrap_or_else(|e| panic!("{e}"));
    for (id, content, vector) in &vectors {
        let word = content.split_whitespace().last().unwrap();
        let got = store.recall(&q(word, 5, None)).await.unwrap();
        let m = got.iter().find(|m| m.id == *id).unwrap_or_else(|| panic!("{id} not recalled"));
        assert_eq!(m.embedding.as_deref(), Some(&vector[..]), "{id}: the vector equals the original");
        assert!(m.embedding.as_ref().unwrap().iter().all(|f| !f.is_nan()), "{id}: no NaN");
    }
    drop(store);

    // After the migration a wrong passphrase still changes nothing.
    let migrated = snapshot(&path).await;
    assert!(open(&path, "wrong").await.is_err());
    assert_eq!(snapshot(&path).await, migrated);
}

#[tokio::test]
async fn test_g25_a_fresh_database_should_reject_a_wrong_passphrase_on_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"))); // writes the salt sentinel (and the key check)
    let before = snapshot(&path).await;

    let wrong = open(&path, "other").await;

    assert!(wrong.is_err(), "no memories to trial-decrypt: the key-check value must catch it");
    drop(wrong);
    assert_eq!(snapshot(&path).await, before);
    assert!(open(&path, "pw").await.is_ok(), "the right passphrase still opens it");
}

// ---- G26 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g26_sealed_vectors_should_start_with_the_eight_byte_magic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let vector = vec![1.5f32, 0.25, 0.125, 0.0625];
    {
        let store = open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"));
        let mut m = mem("m", "note about quokkas", MemoryKind::Feedback, 1);
        m.embedding = Some(vector.clone());
        store.record(m).await.unwrap();
    }

    let blob = embedding_of(&path, "m").await.expect("a blob");

    assert_eq!(&blob[..8], &MAGIC, "the magic");
    assert!(blob.len() >= 8 + 12 + 16 + 16, "magic + nonce + tag + ciphertext: {} bytes", blob.len());
    // G27 (testable part): today's plain decoder does not read the sealed row as the original vector.
    let plain = SqliteStore::open_without_fts(&path).await.unwrap();
    let seen = plain.recall(&q("", 10, None)).await.unwrap();
    let read = seen.iter().find(|m| m.id == "m").expect("the plain store lists the row");
    assert!(read.content.starts_with("enc:v1:"));
    assert_ne!(read.embedding.as_ref().map(Vec::len), Some(vector.len()), "never a vector of the original dimension");
    assert_ne!(read.embedding.as_deref(), Some(&vector[..]));
}

#[tokio::test]
async fn test_g26_a_legacy_vector_starting_with_0xe1_should_be_re_encrypted_with_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let vectors = legacy_vectors();
    legacy_db(&path, "pw", &vectors).await;
    let plain_e1 = f32_bytes(&e1_vector());
    assert_eq!(embedding_of(&path, "e1").await.as_deref(), Some(&plain_e1[..]), "harness: the 0xE1 vector is plaintext");

    let store = open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"));

    for (id, _, v) in vectors.iter().rev() {
        // (the 0xE1 vector first: it is the one the 1-byte marker missed)
        let blob = embedding_of(&path, id).await.unwrap();
        assert_eq!(&blob[..8], &MAGIC, "{id}: sealed (magic-prefixed)");
        assert_ne!(blob, f32_bytes(v), "{id}: no longer plaintext");
    }
    let got = store.recall(&q("gamma", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&e1_vector()[..]), "and it decrypts to the original");
    drop(store);
    // Idempotent: reopening leaves sealed blobs alone.
    let sealed = snapshot(&path).await;
    drop(open(&path, "pw").await.unwrap());
    assert_eq!(snapshot(&path).await, sealed);

    // Blobs written by earlier builds of this branch (1-byte marker) still decrypt.
    let old = dir.path().join("marker1.db");
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/enc_marker1.db"), &old).unwrap();
    assert_eq!(embedding_of(&old, "sealed-row").await.unwrap()[0], 0xE1, "harness: a 1-byte-marker blob");
    let store = open(&old, "fixture-pass").await.unwrap_or_else(|e| panic!("{e}"));
    let got = store.recall(&q("quokkas", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[1.0f32, 0.0, 0.5, 0.25][..]), "earlier sealed blobs decrypt");
    let scored = store.recall_semantic(&[1.0, 0.0, 0.5, 0.25], 2).await.unwrap();
    assert_eq!(scored[0].memory.id, "sealed-row");
}
