#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G29-G32 of `docs/plans/fts5-recall-spec.md` (review round 4): sealed vectors in a side
//! table, a targeted migration, concurrent first opens, and verification without a key check.
//!
//! Contract pinned:
//! - G29: each encrypted memory's embedding is stored sealed (12-byte nonce + AES-GCM
//!   ciphertext of the LE f32s) in `memory_vectors_sealed(id TEXT PRIMARY KEY, blob BLOB NOT
//!   NULL)`; `memories.embedding` is NULL for encrypted rows; forgetting or replacing a memory
//!   removes or replaces its sealed row; a plain `SqliteStore` listing sees no embedding.
//! - G30: on open, `enc:v1` rows with a non-NULL `embedding` column are migrated (plaintext
//!   vectors and blobs of earlier builds, 8-byte magic or 1-byte marker): sealed into the side
//!   table, the column set to NULL. An open with nothing to migrate performs no UPDATE of
//!   `memories`. A magic-prefixed blob that fails to decrypt is left in place, skipped (the open
//!   still succeeds), and reads as no embedding, never as a vector.
//! - G31: two concurrent first opens of one fresh database: with the same passphrase both
//!   succeed and every row written by either is readable after reopening; with different
//!   passphrases exactly one succeeds.
//! - G32: a database written by `main` (the salt sentinel has no key check: description NULL)
//!   opened with a wrong passphrase is an error that changes nothing (memories, vectors, side
//!   table, meta); the right passphrase then works and writes the key check (the sentinel's
//!   description becomes non-NULL). Trial decryption tries several encrypted rows, so a corrupted
//!   first row (encrypted under another salt) does not reject the right passphrase. A sentinel-
//!   only database pins the first passphrase used.
//!
//! Legacy databases are built the way `main` writes them: `enc:v1` content, plaintext f32 LE
//! bytes in the `embedding` column, a sentinel with `description` NULL.

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, MemoryFilter, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

const SENTINEL: &str = "__encrypted_store_salt__";
const MAGIC: [u8; 8] = [0xE1, 0x48, 0x4F, 0x4E, 0x45, 0x56, 0x31, 0x00];

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

async fn column(path: &Path, id: &str) -> Option<Vec<u8>> {
    let p = pool(path).await;
    let v = sqlx::query("SELECT embedding FROM memories WHERE id = ?1").bind(id).fetch_one(&p).await.unwrap().get::<Option<Vec<u8>>, _>(0);
    p.close().await;
    v
}

async fn sealed(path: &Path, id: &str) -> Option<Vec<u8>> {
    let p = pool(path).await;
    let r = sqlx::query("SELECT blob FROM memory_vectors_sealed WHERE id = ?1").bind(id).fetch_optional(&p).await;
    p.close().await;
    r.ok().flatten().map(|r| r.get::<Vec<u8>, _>(0))
}

/// Everything in the database that a failed open must not change, as comparable text.
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

// ---- G29 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g29_encrypted_rows_should_keep_the_embedding_column_null_and_seal_into_the_side_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let vector = vec![1.5f32, 0.25, 0.125, 0.0625];
    {
        let store = open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"));
        let mut m = mem("with", "note about quokkas", MemoryKind::Feedback, 1);
        m.embedding = Some(vector.clone());
        m.tags = vec!["doomed".to_owned()];
        store.record(m).await.unwrap();
        store.record(mem("without", "note about narwhals", MemoryKind::Feedback, 2)).await.unwrap();
    }

    assert!(column(&path, "with").await.is_none(), "memories.embedding is NULL");
    let blob = sealed(&path, "with").await.expect("a sealed row");
    assert!(blob.len() >= 12 + 16, "nonce + ciphertext: {}", blob.len());
    assert!(!blob.windows(16).any(|w| w == f32_bytes(&vector)), "not the plaintext vector");
    assert!(sealed(&path, "without").await.is_none(), "no embedding, no sealed row");
    // An older reader sees no embedding, so no bogus vector.
    let plain = SqliteStore::open_without_fts(&path).await.unwrap();
    let listed = plain.recall(&q("", 10, None)).await.unwrap();
    assert!(listed.iter().all(|m| m.embedding.is_none()), "no embedding visible to a plain store");
    drop(plain);

    // Replacing and forgetting keep the side table in step.
    let store = open(&path, "pw").await.unwrap();
    let mut again = mem("with", "note about quokkas, revised", MemoryKind::Feedback, 3);
    again.embedding = Some(vec![9.0, 8.0, 7.0, 6.0]);
    again.tags = vec!["doomed".to_owned()];
    store.record(again).await.unwrap();
    let replaced = sealed(&path, "with").await.expect("still one sealed row");
    assert_ne!(replaced, blob, "the sealed vector was replaced");
    let got = store.recall(&q("revised", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[9.0f32, 8.0, 7.0, 6.0][..]));
    let mut plain_again = mem("with", "no vector any more", MemoryKind::Feedback, 4);
    plain_again.tags = vec!["doomed".to_owned()];
    store.record(plain_again).await.unwrap();
    assert!(sealed(&path, "with").await.is_none(), "replacing with a memory without a vector removes its sealed row");
    let mut with_again = mem("with", "vector again", MemoryKind::Feedback, 5);
    with_again.embedding = Some(vec![1.0, 1.0, 1.0, 1.0]);
    with_again.tags = vec!["doomed".to_owned()];
    store.record(with_again).await.unwrap();
    assert!(sealed(&path, "with").await.is_some());
    store.forget(&MemoryFilter::Tag("doomed".to_owned())).await.unwrap();
    assert!(sealed(&path, "with").await.is_none(), "forgetting removes the sealed row");
}

// ---- G30 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g30_a_main_format_database_should_migrate_on_the_right_passphrase_and_then_stay_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "pw", &vectors()).await;
    assert_eq!(column(&path, "a").await.as_deref(), Some(&f32_bytes(&[0.5, 0.5, 0.25, 0.125])[..]), "harness: plaintext column");

    let store = open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"));

    for (id, _, v) in vectors() {
        match v {
            Some(v) => {
                assert!(column(&path, id).await.is_none(), "{id}: column NULL");
                let blob = sealed(&path, id).await.unwrap_or_else(|| panic!("{id}: sealed row missing"));
                assert_ne!(blob, f32_bytes(&v));
            }
            None => assert!(sealed(&path, id).await.is_none()),
        }
    }
    let got = store.recall(&q("alpha", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]));
    drop(store);

    // Nothing left to migrate: a further open performs no UPDATE of `memories`.
    let p = pool(&path).await;
    sqlx::query("CREATE TABLE IF NOT EXISTS touch_log (id TEXT)").execute(&p).await.unwrap();
    sqlx::query("CREATE TRIGGER IF NOT EXISTS touch_log_au AFTER UPDATE ON memories BEGIN INSERT INTO touch_log VALUES (old.id); END").execute(&p).await.unwrap();
    p.close().await;
    drop(open(&path, "pw").await.unwrap());
    let p = pool(&path).await;
    let touched: Vec<String> = sqlx::query("SELECT id FROM touch_log").fetch_all(&p).await.unwrap().iter().map(|r| r.get(0)).collect();
    p.close().await;
    assert!(touched.is_empty(), "an open with nothing to migrate must not rewrite rows: {touched:?}");
}

#[tokio::test]
async fn test_g30_blobs_from_earlier_builds_should_migrate_into_the_side_table() {
    for (fixture, label) in [("enc_marker1.db", "1-byte marker"), ("enc_magic8.db", "8-byte magic")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        std::fs::copy(format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR")), &path).unwrap();
        let id = "sealed-row";
        assert_eq!(column(&path, id).await.unwrap()[0], 0xE1, "{label}: harness: a blob in the column");

        let store = open(&path, "fixture-pass").await.unwrap_or_else(|e| panic!("{label}: {e}"));

        assert!(column(&path, id).await.is_none(), "{label}: column NULL");
        assert!(sealed(&path, id).await.is_some(), "{label}: sealed row");
        let got = store.recall(&q("quokkas", 5, None)).await.unwrap();
        assert_eq!(got[0].embedding.as_deref(), Some(&[1.0f32, 0.0, 0.5, 0.25][..]), "{label}: decrypts");
        let scored = store.recall_semantic(&[1.0, 0.0, 0.5, 0.25], 2).await.unwrap();
        assert_eq!(scored[0].memory.id, id, "{label}: semantic recall");
    }
}

#[tokio::test]
async fn test_g30_a_magic_blob_that_fails_to_decrypt_should_stay_be_skipped_and_read_as_no_embedding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "pw", &vectors()).await;
    // A blob that looks sealed (magic) but is not valid under this key.
    let mut bad = MAGIC.to_vec();
    bad.extend((0..44u8).map(|i| i.wrapping_mul(7).wrapping_add(3)));
    let p = pool(&path).await;
    sqlx::query("UPDATE memories SET embedding = ?1 WHERE id = 'b'").bind(&bad).execute(&p).await.unwrap();
    p.close().await;

    let store = open(&path, "pw").await.expect("an undecryptable blob must not fail the open");

    assert_eq!(column(&path, "b").await.as_deref(), Some(&bad[..]), "left in place, never re-sealed as if plaintext");
    assert!(sealed(&path, "b").await.is_none(), "no sealed row for it");
    let got = store.recall(&q("beta", 5, None)).await.unwrap();
    assert_eq!(got[0].id, "b");
    assert!(got[0].embedding.is_none(), "reads as no embedding, never as a vector: {:?}", got[0].embedding);
    assert!(column(&path, "a").await.is_none(), "the other rows are migrated");
    assert!(sealed(&path, "a").await.is_some());
}

// ---- G31 ---------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_g31_two_concurrent_first_opens_with_one_passphrase_should_both_succeed_and_keep_every_row() {
    for round in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let (a, b) = (SqliteStore::open_without_fts(&path).await.unwrap(), SqliteStore::open_without_fts(&path).await.unwrap());
        let write = |inner: SqliteStore, id: &'static str, word: &'static str| {
            tokio::spawn(async move {
                let store = EncryptedStore::new(inner, "shared").await?;
                let mut m = mem(id, &format!("note about {word}"), MemoryKind::Feedback, 1);
                m.embedding = Some(vec![1.0, 2.0, 3.0, 4.0]);
                store.record(m).await
            })
        };

        let (ra, rb) = (write(a, "from-a", "alpacas"), write(b, "from-b", "quokkas"));

        ra.await.unwrap().unwrap_or_else(|e| panic!("round {round}: first open or write failed: {e}"));
        rb.await.unwrap().unwrap_or_else(|e| panic!("round {round}: second open or write failed: {e}"));
        let store = open(&path, "shared").await.unwrap_or_else(|e| panic!("round {round}: reopen: {e}"));
        for (word, id) in [("alpacas", "from-a"), ("quokkas", "from-b")] {
            let got = store.recall(&q(word, 5, None)).await.unwrap_or_else(|e| panic!("round {round}: {word}: {e}"));
            assert_eq!(got.first().map(|m| m.id.as_str()), Some(id), "round {round}: {word}");
            assert_eq!(got[0].embedding.as_deref(), Some(&[1.0f32, 2.0, 3.0, 4.0][..]), "round {round}: {id} decrypts with its vector");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_g31_two_concurrent_first_opens_with_different_passphrases_should_let_exactly_one_win() {
    for round in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        let (a, b) = (SqliteStore::open_without_fts(&path).await.unwrap(), SqliteStore::open_without_fts(&path).await.unwrap());

        let ra = tokio::spawn(async move { EncryptedStore::new(a, "first-passphrase").await.map(|_| ()) });
        let rb = tokio::spawn(async move { EncryptedStore::new(b, "second-passphrase").await.map(|_| ()) });

        let (ra, rb) = (ra.await.unwrap(), rb.await.unwrap());
        assert_eq!(
            [ra.is_ok(), rb.is_ok()].iter().filter(|ok| **ok).count(),
            1,
            "round {round}: exactly one passphrase wins, the other gets an error (got {:?} / {:?})",
            ra.err().map(|e| e.to_string()),
            rb.err().map(|e| e.to_string())
        );
    }
}

// ---- G32 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g32_a_wrong_passphrase_on_a_main_format_database_should_change_nothing_and_the_right_one_writes_the_key_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    main_style_db(&path, "pw", &vectors()).await;
    let before = full_snapshot(&path).await;
    {
        let p = pool(&path).await;
        let d: Option<String> = sqlx::query("SELECT description FROM memories WHERE id = ?1").bind(SENTINEL).fetch_one(&p).await.unwrap().get(0);
        p.close().await;
        assert!(d.is_none(), "harness: the sentinel has no key check (description NULL)");
    }

    let wrong = open(&path, "wrong").await;

    assert!(wrong.is_err(), "a wrong passphrase is an error");
    drop(wrong);
    assert_eq!(full_snapshot(&path).await, before, "content, description, weight, vectors, side table and meta unchanged");

    let store = open(&path, "pw").await.unwrap_or_else(|e| panic!("{e}"));
    let got = store.recall(&q("alpha", 5, None)).await.unwrap();
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.5, 0.25, 0.125][..]));
    drop(store);
    let p = pool(&path).await;
    let description: Option<String> = sqlx::query("SELECT description FROM memories WHERE id = ?1").bind(SENTINEL).fetch_one(&p).await.unwrap().get(0);
    p.close().await;
    assert!(description.is_some_and(|d| !d.is_empty()), "the key check was written on the first successful open");
    assert!(open(&path, "wrong").await.is_err(), "and now a wrong passphrase is rejected by it");

    // A sentinel-only database (main-style, no memories) pins the first passphrase used.
    let only = tempfile::tempdir().unwrap();
    let path = only.path().join("memory.db");
    main_style_db(&path, "first", &[]).await; // main-style: a sentinel, no memories, no key check

    let first = open(&path, "first").await;
    assert!(first.is_ok(), "the first passphrase is accepted and pinned: {:?}", first.err().map(|e| e.to_string()));
    drop(first);

    assert!(open(&path, "second").await.is_err(), "a different passphrase afterwards is rejected");
    assert!(open(&path, "first").await.is_ok());
}

#[tokio::test]
async fn test_g32_trial_decryption_should_accept_the_right_passphrase_when_some_sample_rows_are_corrupted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let rows: Vec<(&str, &str, Option<Vec<f32>>)> = vec![
        ("r1", "note about alpha", None),
        ("r2", "note about beta", None),
        ("r3", "note about gamma", None),
        ("r4", "note about delta", None),
        ("r5", "note about epsilon", None),
    ];
    main_style_db(&path, "pw", &rows).await;
    // A foreign ciphertext (another database, so another salt): decryptable by no key of this one.
    let other = dir.path().join("other.db");
    let foreign = {
        let store = open(&other, "other-pw").await.unwrap();
        store.record(mem("x", "foreign text", MemoryKind::Feedback, 1)).await.unwrap();
        drop(store);
        let p = pool(&other).await;
        let c: String = sqlx::query("SELECT content FROM memories WHERE id = 'x'").fetch_one(&p).await.unwrap().get(0);
        p.close().await;
        c
    };
    // Corrupt the oldest and the newest sample candidates, whatever order the check samples in.
    let p = pool(&path).await;
    sqlx::query("UPDATE memories SET content = ?1, created_at = 0 WHERE id = 'r1'").bind(&foreign).execute(&p).await.unwrap();
    sqlx::query("UPDATE memories SET content = ?1, created_at = 9999999999 WHERE id = 'r5'").bind(&foreign).execute(&p).await.unwrap();
    p.close().await;

    let right = open(&path, "pw").await;
    assert!(right.is_ok(), "the right passphrase is accepted: three of five rows decrypt: {:?}", right.err().map(|e| e.to_string()));
    drop(right);
    assert!(open(&path, "wrong").await.is_err(), "a wrong one still fails");
}
