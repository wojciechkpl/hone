#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! ES4, ES5 and ES6 of `docs/plans/encrypted-store-safety-spec.md` (review round 1).
//!
//! ES4: `EncryptedStore::new` finds the salt sentinel by id (not by listing the newest 10,000
//! memories), decides "are there encrypted rows?" by a direct query, and never replaces an
//! existing sentinel; with 10,000+ newer rows or 10,001+ encrypted rows the store still opens
//! with its original salt.
//!
//! ES5: a description that cannot be decrypted reads back as `None` (never as `enc:v1:...`);
//! content that cannot be decrypted still errors.
//!
//! ES6: `EncryptedStore::forget` / `decay` never remove the sentinel from the inner store,
//! including an `InMemoryStore`. The inner store is wrapped in a delegating `Shared` so the
//! tests can look at it and reopen over it. No new API.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use hone_memory::{
    EncryptedStore, InMemoryStore, Memory, MemoryError, MemoryFilter, MemoryKind, MemoryQuery,
    MemoryStore, SqliteStore,
};

const SENTINEL: &str = "__encrypted_store_salt__";
const PASS: &str = "correct horse battery staple";
const OTHER_SALT: &str = "QUFBQUFBQUFBQUFBQUFBQQ=="; // 16 bytes of 'A'

fn mem(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

fn all_query() -> MemoryQuery {
    MemoryQuery { text: String::new(), limit: 100_000, kind: None }
}

async fn plain(path: &Path) -> SqliteStore {
    SqliteStore::open(path).await.unwrap()
}

async fn raw_salt(path: &Path) -> Option<String> {
    plain(path).await.recall_by_id(SENTINEL).await.unwrap().map(|m| m.content)
}

async fn open_encrypted(path: &Path) -> Result<EncryptedStore<SqliteStore>, MemoryError> {
    EncryptedStore::new(plain(path).await, PASS).await
}

/// Inserts `n` rows with `created_at` from `first_created_at` up, in one transaction.
async fn bulk_insert(path: &Path, n: u64, first_created_at: u64, content: impl Fn(u64) -> String) {
    use sqlx::Connection;
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let mut conn = sqlx::SqliteConnection::connect(&url).await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    for i in 0..n {
        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
             VALUES (?1, ?2, '[]', 1.0, ?3, NULL, 'conversation', NULL, NULL)",
        )
        .bind(format!("bulk-{i:06}"))
        .bind(content(i))
        .bind((first_created_at + i) as i64)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    conn.close().await.unwrap();
}

// ---- ES4: the salt is found by id and never overwritten ----------------------------------------------

#[tokio::test]
async fn test_es4_ten_thousand_newer_plaintext_rows_should_not_cost_the_store_its_salt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let enc = open_encrypted(&path).await.unwrap();
        enc.record(mem("old", "an old encrypted memory", MemoryKind::Feedback, 1_700_000_000)).await.unwrap();
    }
    let salt_before = raw_salt(&path).await.expect("the salt exists");
    // A plain run (no --encrypted-memory) on the same database writes 10,000 newer rows.
    bulk_insert(&path, 10_000, 1_800_000_000, |i| format!("plaintext row {i}")).await;

    let enc = open_encrypted(&path).await.expect("opens with the original salt");

    assert_eq!(raw_salt(&path).await.unwrap(), salt_before, "the salt is unchanged byte for byte");
    let old = enc.recall_by_id("old").await.unwrap().expect("the memory is there");
    assert_eq!(old.content, "an old encrypted memory", "and still decrypts");
}

#[tokio::test]
async fn test_es4_more_than_ten_thousand_encrypted_rows_should_not_stop_the_store_opening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let enc = open_encrypted(&path).await.unwrap();
        enc.record(mem("oldest", "the oldest secret", MemoryKind::Feedback, 1_000)).await.unwrap();
    }
    let salt_before = raw_salt(&path).await.unwrap();
    // 10,000 more rows that look encrypted (only their prefix matters to the open check)...
    bulk_insert(&path, 10_000, 1_700_000_000, |i| format!("enc:v1:{i:016}")).await;
    {
        // ...and the newest real one, written by the encrypted store itself.
        let enc = open_encrypted(&path).await.expect("opens");
        enc.record(mem("newest", "the newest secret", MemoryKind::Feedback, 1_900_000_000)).await.unwrap();
    }

    let enc = open_encrypted(&path).await.expect("opens with more than 10,000 encrypted rows");

    assert_eq!(raw_salt(&path).await.unwrap(), salt_before);
    assert_eq!(enc.recall_by_id("oldest").await.unwrap().unwrap().content, "the oldest secret");
    assert_eq!(enc.recall_by_id("newest").await.unwrap().unwrap().content, "the newest secret");
}

#[tokio::test]
async fn test_es4_an_existing_sentinel_should_never_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = plain(&path).await;
    store
        .record(Memory {
            id: SENTINEL.to_owned(),
            content: OTHER_SALT.to_owned(),
            tags: vec!["__internal__".to_owned()],
            weight: 0.0,
            created_at: 0,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: None,
        })
        .await
        .unwrap();
    drop(store);
    bulk_insert(&path, 10_000, 1_800_000_000, |i| format!("plaintext row {i}")).await;

    let _enc = open_encrypted(&path).await.expect("opens");

    assert_eq!(raw_salt(&path).await.unwrap(), OTHER_SALT, "the existing salt was not overwritten");
}

#[tokio::test]
async fn test_es4_a_missing_sentinel_with_no_encrypted_rows_should_be_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = plain(&path).await;
    store.record(mem("p1", "plain one", MemoryKind::Feedback, 100)).await.unwrap();
    store.record(mem("p2", "plain two", MemoryKind::Feedback, 200)).await.unwrap();
    drop(store);
    assert!(raw_salt(&path).await.is_none());

    let enc = open_encrypted(&path).await.expect("a store without encrypted rows may be adopted");

    let salt = raw_salt(&path).await.expect("the sentinel was created");
    assert!(!salt.is_empty());
    enc.record(mem("e1", "first secret", MemoryKind::Feedback, 300)).await.unwrap();
    drop(enc);
    let again = open_encrypted(&path).await.unwrap();
    assert_eq!(again.recall_by_id("e1").await.unwrap().unwrap().content, "first secret");
    assert_eq!(raw_salt(&path).await.unwrap(), salt, "the same salt on every open");
}

#[tokio::test]
async fn test_es4_a_missing_sentinel_with_encrypted_rows_should_still_refuse_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let enc = open_encrypted(&path).await.unwrap();
        enc.record(mem("e1", "secret", MemoryKind::Feedback, 100)).await.unwrap();
    }
    // The sentinel is really gone (not just out of a listing).
    sqlx_delete_sentinel(&path).await;

    let err = open_encrypted(&path).await.err().expect("must refuse, not invent a new salt");

    assert!(err.to_string().contains("salt sentinel"), "{err}");
}

async fn sqlx_delete_sentinel(path: &Path) {
    use sqlx::Connection;
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let mut conn = sqlx::SqliteConnection::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM memories WHERE id = ?1").bind(SENTINEL).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
}

// ---- ES5: undecryptable descriptions ------------------------------------------------------------------------

async fn store_with_description(path: &Path) {
    let enc = open_encrypted(path).await.unwrap();
    let mut m = mem("d1", "the content is fine", MemoryKind::Feedback, 1_700_000_000);
    m.description = Some("a description worth encrypting".to_owned());
    enc.record(m).await.unwrap();
}

async fn corrupt_description(path: &Path, corrupted: &str) {
    let p = plain(path).await;
    let mut row = p.recall_by_id("d1").await.unwrap().unwrap();
    row.description = Some(corrupted.to_owned());
    p.record(row).await.unwrap();
}

#[tokio::test]
async fn test_es5_a_description_that_fails_to_decrypt_should_read_back_as_none() {
    for corrupted in [
        "enc:v1:bm90LXZhbGlkLWNpcGhlcg==", // valid base64, not a valid ciphertext
        "enc:v1:@@@ not base64 @@@",
        "enc:v1:",
        "enc:v1:AAAA", // too short
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        store_with_description(&path).await;
        corrupt_description(&path, corrupted).await;
        let enc = open_encrypted(&path).await.unwrap();

        let by_id = enc.recall_by_id("d1").await.unwrap().expect("the memory is still returned");
        assert_eq!(by_id.description, None, "{corrupted:?}: not shown as ciphertext");
        assert_eq!(by_id.content, "the content is fine", "content intact");

        let listed = enc.recall(&all_query()).await.unwrap();
        let d1 = listed.iter().find(|m| m.id == "d1").expect("listed");
        assert_eq!(d1.description, None, "{corrupted:?}");
        assert_eq!(d1.content, "the content is fine");
    }
}

#[tokio::test]
async fn test_es5_a_description_encrypted_under_another_key_should_read_back_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    store_with_description(&path).await;
    // The ciphertext of a different store (other salt, other key).
    let other = dir.path().join("other.db");
    store_with_description(&other).await;
    let foreign = plain(&other).await.recall_by_id("d1").await.unwrap().unwrap().description.unwrap();
    assert!(foreign.starts_with("enc:v1:"));
    corrupt_description(&path, &foreign).await;
    let enc = open_encrypted(&path).await.unwrap();

    let got = enc.recall_by_id("d1").await.unwrap().unwrap();

    assert_eq!(got.description, None);
}

#[tokio::test]
async fn test_es5_content_that_fails_to_decrypt_should_still_be_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    store_with_description(&path).await;
    let p = plain(&path).await;
    let mut row = p.recall_by_id("d1").await.unwrap().unwrap();
    row.content = "enc:v1:bm90LXZhbGlkLWNpcGhlcg==".to_owned();
    p.record(row).await.unwrap();
    drop(p);
    let enc = open_encrypted(&path).await.unwrap();

    assert!(enc.recall_by_id("d1").await.is_err(), "unchanged: bad content is an error");
}

#[tokio::test]
async fn test_es5_an_intact_description_should_still_decrypt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    store_with_description(&path).await;
    let enc = open_encrypted(&path).await.unwrap();
    let got = enc.recall_by_id("d1").await.unwrap().unwrap();
    assert_eq!(got.description.as_deref(), Some("a description worth encrypting"));
}

// ---- ES6: every inner store ---------------------------------------------------------------------------------------

/// Delegates to a shared `InMemoryStore`, so the test can inspect and reopen over it.
struct Shared(Arc<InMemoryStore>);

#[async_trait]
impl MemoryStore for Shared {
    async fn record(&self, m: Memory) -> Result<(), MemoryError> {
        self.0.record(m).await
    }
    async fn recall(&self, q: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        self.0.recall(q).await
    }
    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        self.0.recall_by_id(id).await
    }
    async fn forget(&self, f: &MemoryFilter) -> Result<u32, MemoryError> {
        self.0.forget(f).await
    }
    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        self.0.decay(factor, now).await
    }
    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        self.0.export(path).await
    }
    async fn import(_path: &Path) -> Result<Self, MemoryError> {
        Err(MemoryError::Io(std::io::Error::other("not supported")))
    }
}

async fn encrypted_in_memory() -> (EncryptedStore<Shared>, Arc<InMemoryStore>) {
    let inner = Arc::new(InMemoryStore::new());
    let enc = EncryptedStore::new(Shared(inner.clone()), PASS).await.unwrap();
    enc.record(mem("e1", "secret one", MemoryKind::Conversation, 1_700_000_001)).await.unwrap();
    enc.record(mem("e2", "secret two", MemoryKind::Feedback, 1_700_000_002)).await.unwrap();
    enc.record(mem("e3", "secret three", MemoryKind::User, 1_700_000_003)).await.unwrap();
    (enc, inner)
}

async fn reopen(inner: &Arc<InMemoryStore>) -> EncryptedStore<Shared> {
    EncryptedStore::new(Shared(inner.clone()), PASS)
        .await
        .unwrap_or_else(|e| panic!("the store must reopen over the same inner store: {e}"))
}

#[tokio::test]
async fn test_es6_forget_before_should_keep_the_sentinel_in_an_in_memory_inner_store() {
    let (enc, inner) = encrypted_in_memory().await;

    let n = enc.forget(&MemoryFilter::Before(2_000_000_000)).await.unwrap();

    assert_eq!(n, 3, "the sentinel is not counted");
    assert!(inner.recall_by_id(SENTINEL).await.unwrap().is_some(), "the salt is still stored");
    let again = reopen(&inner).await;
    assert!(again.recall(&all_query()).await.unwrap().is_empty());
}

#[tokio::test]
async fn test_es6_forget_by_conversation_kind_should_keep_the_sentinel_and_the_other_memories() {
    let (enc, inner) = encrypted_in_memory().await;

    let n = enc.forget(&MemoryFilter::Kind(MemoryKind::Conversation)).await.unwrap();

    assert_eq!(n, 1);
    assert!(inner.recall_by_id(SENTINEL).await.unwrap().is_some());
    let again = reopen(&inner).await;
    let mut got: Vec<String> = again.recall(&all_query()).await.unwrap().into_iter().map(|m| m.content).collect();
    got.sort();
    assert_eq!(got, ["secret three", "secret two"]);
}

#[tokio::test]
async fn test_es6_forget_after_should_keep_the_sentinel() {
    let (enc, inner) = encrypted_in_memory().await;

    let n = enc.forget(&MemoryFilter::After(0)).await.unwrap();

    assert_eq!(n, 3);
    assert!(inner.recall_by_id(SENTINEL).await.unwrap().is_some());
}

#[tokio::test]
async fn test_es6_forget_by_the_sentinels_tag_should_keep_it() {
    let (enc, inner) = encrypted_in_memory().await;

    let n = enc.forget(&MemoryFilter::Tag("__internal__".to_owned())).await.unwrap();

    assert_eq!(n, 0);
    assert!(inner.recall_by_id(SENTINEL).await.unwrap().is_some());
    assert_eq!(reopen(&inner).await.recall(&all_query()).await.unwrap().len(), 3);
}

#[tokio::test]
async fn test_es6_decay_expiry_should_keep_the_sentinel_in_an_in_memory_inner_store() {
    let (enc, inner) = encrypted_in_memory().await;
    // Even a sentinel that somehow carries an expiry must survive the decay pass.
    let mut s = inner.recall_by_id(SENTINEL).await.unwrap().unwrap();
    s.decay_at = Some(5);
    inner.record(s).await.unwrap();

    let removed = enc.decay(1.0, 10).await.unwrap();

    assert_eq!(removed, 0, "nothing ordinary expired; the sentinel is not counted");
    assert!(inner.recall_by_id(SENTINEL).await.unwrap().is_some());
    assert_eq!(reopen(&inner).await.recall(&all_query()).await.unwrap().len(), 3);
}
