#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G36 (runtime part, no trait import: behaviour-level) of `docs/plans/fts5-recall-spec.md`
//! review round 5: `EncryptedStore::record` never reports an error AFTER the row was written,
//! with and without an embedding, over `SqliteStore` and `InMemoryStore`; and what it wrote
//! reads back (content and vector).

mod fts5_support;

use fts5_support::{mem, q};
use hone_memory::{EncryptedStore, InMemoryStore, MemoryKind, MemoryStore, SqliteStore};

macro_rules! check_record_then_read {
    ($store:expr, $label:expr) => {{
        let store = $store;
        let label: &str = $label;
    let mut with = mem("with", "a note about quokkas", MemoryKind::Feedback, 10);
    with.embedding = Some(vec![0.25, 0.5, 0.75, 1.0]);
    let without = mem("without", "a note about wombats", MemoryKind::Feedback, 11);

    store.record(with).await.unwrap_or_else(|e| panic!("{label}: record with an embedding failed after writing: {e}"));
    store.record(without).await.unwrap_or_else(|e| panic!("{label}: record without an embedding failed: {e}"));

    let w = store.recall_by_id("with").await.unwrap().expect("the row is there");
    assert_eq!(w.content, "a note about quokkas", "{label}");
    assert_eq!(w.embedding.as_deref(), Some(&[0.25f32, 0.5, 0.75, 1.0][..]), "{label}: the vector reads back");
    let wo = store.recall_by_id("without").await.unwrap().expect("the row is there");
    assert!(wo.embedding.is_none(), "{label}");
    let listed = store.recall(&q("quokkas", 5, None)).await.unwrap();
    assert_eq!(listed[0].id, "with", "{label}");
    }};
}

#[tokio::test]
async fn test_g36_record_should_never_error_after_writing_over_a_sqlite_store() {
    let inner = SqliteStore::in_memory().await.unwrap();
    check_record_then_read!(EncryptedStore::new(inner, "pw").await.unwrap(), "SqliteStore");
}

#[tokio::test]
async fn test_g36_record_should_never_error_after_writing_over_an_in_memory_store() {
    let inner = InMemoryStore::new();
    check_record_then_read!(EncryptedStore::new(inner, "pw").await.unwrap(), "InMemoryStore");
}

#[tokio::test]
async fn test_g36_re_recording_a_row_without_a_vector_should_drop_its_old_vector() {
    let store = EncryptedStore::new(SqliteStore::in_memory().await.unwrap(), "pw").await.unwrap();
    let mut m = mem("x", "first version", MemoryKind::Feedback, 1);
    m.embedding = Some(vec![1.0, 2.0, 3.0, 4.0]);
    store.record(m).await.unwrap();

    store.record(mem("x", "second version", MemoryKind::Feedback, 2)).await.unwrap();

    let got = store.recall_by_id("x").await.unwrap().unwrap();
    assert_eq!(got.content, "second version");
    assert!(got.embedding.is_none(), "{:?}", got.embedding);
}
