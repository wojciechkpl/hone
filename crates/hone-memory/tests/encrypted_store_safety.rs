#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! ES1 and ES2 of `docs/plans/encrypted-store-safety-spec.md`.
//!
//! ES1: `SqliteStore::forget` never deletes the salt sentinel
//! (`__encrypted_store_salt__`), whatever the filter, and never counts it; the decay
//! pass's expiry never deletes it either. (`MemoryFilter` has no `Id` or "all" variant on
//! `main`: `Before`, `After`, `Kind` and `Tag` are covered; a new variant must keep the
//! guarantee.)
//!
//! ES2: `EncryptedStore` stores a memory's `description` as `enc:v1:...` and returns the
//! plaintext through recall, `recall_by_id`, export and `find_similar`; a plaintext
//! description (written by an older version) reads back unchanged. No new API.

use std::path::Path;

use hone_memory::{
    EncryptedStore, Memory, MemoryFilter, MemoryKind, MemoryQuery, MemoryStore, SqliteStore,
};

const SENTINEL: &str = "__encrypted_store_salt__";
const PASS: &str = "correct horse battery staple";

fn mem(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec!["t".to_owned()],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

fn sentinel(decay_at: Option<u64>) -> Memory {
    Memory {
        id: SENTINEL.to_owned(),
        content: "c2FsdHNhbHRzYWx0c2FsdA==".to_owned(),
        tags: vec!["__internal__".to_owned()],
        weight: 0.0,
        created_at: 0,
        decay_at,
        kind: MemoryKind::Conversation,
        description: None,
        embedding: None,
    }
}

fn all_query() -> MemoryQuery {
    MemoryQuery { text: String::new(), limit: 1000, kind: None }
}

async fn plain_with_sentinel() -> SqliteStore {
    let s = SqliteStore::in_memory().await.unwrap();
    s.record(sentinel(None)).await.unwrap();
    s.record(mem("m1", "one", MemoryKind::Conversation, 100)).await.unwrap();
    s.record(mem("m2", "two", MemoryKind::Feedback, 200)).await.unwrap();
    s.record(mem("m3", "three", MemoryKind::User, 300)).await.unwrap();
    s
}

async fn ids(s: &SqliteStore) -> Vec<String> {
    let mut v: Vec<String> = s.recall(&all_query()).await.unwrap().into_iter().map(|m| m.id).collect();
    v.sort();
    v
}

// ---- ES1: forget -----------------------------------------------------------------------------

#[tokio::test]
async fn test_es1_forget_before_should_leave_the_sentinel_and_not_count_it() {
    let s = plain_with_sentinel().await;

    let n = s.forget(&MemoryFilter::Before(1_000)).await.unwrap();

    assert_eq!(n, 3, "the three ordinary memories, not the sentinel");
    assert_eq!(ids(&s).await, [SENTINEL]);
}

#[tokio::test]
async fn test_es1_forget_after_should_leave_the_sentinel_and_not_count_it() {
    let s = plain_with_sentinel().await;

    let n = s.forget(&MemoryFilter::After(0)).await.unwrap();

    assert_eq!(n, 3);
    assert_eq!(ids(&s).await, [SENTINEL]);
}

#[tokio::test]
async fn test_es1_forget_by_the_sentinels_kind_should_leave_it_and_not_count_it() {
    let s = plain_with_sentinel().await;

    let n = s.forget(&MemoryFilter::Kind(MemoryKind::Conversation)).await.unwrap();

    assert_eq!(n, 1, "only m1");
    assert_eq!(ids(&s).await, [SENTINEL, "m2", "m3"]);
}

#[tokio::test]
async fn test_es1_forget_by_the_sentinels_tag_should_leave_it_and_not_count_it() {
    let s = plain_with_sentinel().await;

    let n = s.forget(&MemoryFilter::Tag("__internal__".to_owned())).await.unwrap();

    assert_eq!(n, 0);
    assert_eq!(ids(&s).await, [SENTINEL, "m1", "m2", "m3"]);
}

#[tokio::test]
async fn test_es1_forget_by_an_ordinary_tag_should_still_work() {
    let s = plain_with_sentinel().await;
    assert_eq!(s.forget(&MemoryFilter::Tag("t".to_owned())).await.unwrap(), 3);
    assert_eq!(ids(&s).await, [SENTINEL]);
}

#[tokio::test]
async fn test_es1_decay_expiry_should_never_delete_the_sentinel() {
    let s = SqliteStore::in_memory().await.unwrap();
    s.record(sentinel(Some(5))).await.unwrap(); // even with an expiry in the past
    let mut expiring = mem("m1", "one", MemoryKind::Feedback, 100);
    expiring.decay_at = Some(5);
    s.record(expiring).await.unwrap();

    let removed = s.decay(1.0, 10).await.unwrap();

    assert_eq!(removed, 1, "only the ordinary expired memory is counted");
    assert_eq!(ids(&s).await, [SENTINEL]);
}

// ---- ES1 end to end -------------------------------------------------------------------------------

async fn write_three(path: &Path) {
    let enc = EncryptedStore::new(SqliteStore::open(path).await.unwrap(), PASS).await.unwrap();
    enc.record(mem("e1", "secret one", MemoryKind::Conversation, 1_700_000_001)).await.unwrap();
    enc.record(mem("e2", "secret two", MemoryKind::Feedback, 1_700_000_002)).await.unwrap();
    enc.record(mem("e3", "secret three", MemoryKind::User, 1_700_000_003)).await.unwrap();
}

async fn reopen(path: &Path) -> EncryptedStore<SqliteStore> {
    EncryptedStore::new(SqliteStore::open(path).await.unwrap(), PASS)
        .await
        .unwrap_or_else(|e| panic!("the encrypted store must still open: {e}"))
}

#[tokio::test]
async fn test_es1_a_plain_forget_before_should_leave_an_encrypted_store_openable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    write_three(&path).await;

    // `hone memory forget --before` opens the database as a plain SqliteStore.
    let plain = SqliteStore::open(&path).await.unwrap();
    let n = plain.forget(&MemoryFilter::Before(2_000_000_000)).await.unwrap();
    assert_eq!(n, 3, "the sentinel is not counted");
    drop(plain);

    let enc = reopen(&path).await;
    assert!(enc.recall(&all_query()).await.unwrap().is_empty(), "the three were forgotten");
    // And the store keeps working with the same key.
    enc.record(mem("e4", "new secret", MemoryKind::Feedback, 1_800_000_000)).await.unwrap();
    let got = enc.recall(&all_query()).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].content, "new secret");
}

#[tokio::test]
async fn test_es1_a_plain_forget_by_conversation_kind_should_leave_the_other_memories_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    write_three(&path).await;

    let plain = SqliteStore::open(&path).await.unwrap();
    let n = plain.forget(&MemoryFilter::Kind(MemoryKind::Conversation)).await.unwrap();
    assert_eq!(n, 1, "only e1; not the sentinel");
    drop(plain);

    let enc = reopen(&path).await;
    let mut got: Vec<(String, String)> =
        enc.recall(&all_query()).await.unwrap().into_iter().map(|m| (m.id, m.content)).collect();
    got.sort();
    assert_eq!(
        got,
        [("e2".to_owned(), "secret two".to_owned()), ("e3".to_owned(), "secret three".to_owned())],
        "the feedback and user memories still decrypt"
    );
}

#[tokio::test]
async fn test_es1_a_plain_decay_pass_should_leave_an_encrypted_store_openable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    write_three(&path).await;

    let plain = SqliteStore::open(&path).await.unwrap();
    plain.decay(0.9, 4_000_000_000).await.unwrap();
    drop(plain);

    let enc = reopen(&path).await;
    assert_eq!(enc.recall(&all_query()).await.unwrap().len(), 3);
}

// ---- ES2: descriptions ------------------------------------------------------------------------------

const DESCRIPTION: &str = "feedback rule: my bank PIN is 4321";

async fn store_with_description(path: &Path) -> EncryptedStore<SqliteStore> {
    let enc = EncryptedStore::new(SqliteStore::open(path).await.unwrap(), PASS).await.unwrap();
    let mut m = mem("d1", "the rule text about the cache", MemoryKind::Feedback, 1_700_000_000);
    m.description = Some(DESCRIPTION.to_owned());
    enc.record(m).await.unwrap();
    enc.record(mem("d2", "no description here", MemoryKind::Feedback, 1_700_000_001)).await.unwrap();
    enc
}

async fn raw(path: &Path, id: &str) -> Memory {
    SqliteStore::open(path).await.unwrap().recall_by_id(id).await.unwrap().expect("the row exists")
}

#[tokio::test]
async fn test_es2_the_stored_description_should_be_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let enc = store_with_description(&path).await;
    drop(enc);

    let row = raw(&path, "d1").await;

    let stored = row.description.expect("a description is stored");
    assert!(stored.starts_with("enc:v1:"), "stored as {stored:?}");
    assert!(!stored.contains("PIN") && !stored.contains("4321"), "{stored}");
    assert!(row.content.starts_with("enc:v1:"), "content stays encrypted too");
}

#[tokio::test]
async fn test_es2_a_memory_without_a_description_should_stay_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(store_with_description(&path).await);

    assert_eq!(raw(&path, "d2").await.description, None);
}

#[tokio::test]
async fn test_es2_recall_and_by_id_should_return_the_plaintext_description() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let enc = store_with_description(&path).await;

    let listed = enc.recall(&all_query()).await.unwrap();
    let d1 = listed.iter().find(|m| m.id == "d1").unwrap();
    assert_eq!(d1.description.as_deref(), Some(DESCRIPTION));
    assert_eq!(d1.content, "the rule text about the cache");
    assert_eq!(listed.iter().find(|m| m.id == "d2").unwrap().description, None);

    let by_id = enc.recall_by_id("d1").await.unwrap().unwrap();
    assert_eq!(by_id.description.as_deref(), Some(DESCRIPTION));

    let searched = enc
        .recall(&MemoryQuery { text: "cache".to_owned(), limit: 10, kind: Some(MemoryKind::Feedback) })
        .await
        .unwrap();
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].description.as_deref(), Some(DESCRIPTION));
}

#[tokio::test]
async fn test_es2_recall_after_reopening_should_still_return_the_plaintext_description() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(store_with_description(&path).await);

    let enc = reopen(&path).await;

    let d1 = enc.recall_by_id("d1").await.unwrap().unwrap();
    assert_eq!(d1.description.as_deref(), Some(DESCRIPTION));
}

#[tokio::test]
async fn test_es2_export_should_contain_the_plaintext_description() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let enc = store_with_description(&path).await;
    let out = dir.path().join("export.json");

    enc.export(&out).await.unwrap();

    let exported: Vec<Memory> = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let d1 = exported.iter().find(|m| m.id == "d1").unwrap();
    assert_eq!(d1.description.as_deref(), Some(DESCRIPTION));
    assert!(exported.iter().all(|m| m.id != SENTINEL));
}

#[tokio::test]
async fn test_es2_find_similar_should_return_the_plaintext_description() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let enc = store_with_description(&path).await;

    let found = enc.find_similar("the rule text about the cache", 0.5).await.unwrap().expect("a match");

    assert_eq!(found.id, "d1");
    assert_eq!(found.description.as_deref(), Some(DESCRIPTION));
}

#[tokio::test]
async fn test_es2_a_plaintext_description_from_an_older_version_should_read_back_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(store_with_description(&path).await);
    // Hand-write the row the way an older version did: encrypted content, plaintext description.
    let plain = SqliteStore::open(&path).await.unwrap();
    let mut legacy = plain.recall_by_id("d2").await.unwrap().unwrap();
    assert!(legacy.content.starts_with("enc:v1:"));
    legacy.description = Some("legacy plaintext description".to_owned());
    plain.record(legacy).await.unwrap();
    drop(plain);

    let enc = reopen(&path).await;

    let d2 = enc.recall_by_id("d2").await.unwrap().unwrap();
    assert_eq!(d2.description.as_deref(), Some("legacy plaintext description"));
    assert_eq!(d2.content, "no description here");
    assert_eq!(raw(&path, "d2").await.description.as_deref(), Some("legacy plaintext description"), "not rewritten");
}

#[tokio::test]
async fn test_es2_an_empty_description_should_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let enc = EncryptedStore::new(SqliteStore::open(&path).await.unwrap(), PASS).await.unwrap();
    let mut m = mem("e", "content", MemoryKind::Feedback, 1);
    m.description = Some(String::new());
    enc.record(m).await.unwrap();

    let got = enc.recall_by_id("e").await.unwrap().unwrap();

    assert_eq!(got.description.as_deref(), Some(""));
}
