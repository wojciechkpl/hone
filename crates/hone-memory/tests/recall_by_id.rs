//! Red-phase tests for `MemoryStore::recall_by_id`.
//!
//! These tests will FAIL TO COMPILE on `main` because:
//!   - `MemoryStore` trait has no `recall_by_id` method.
//!
//! Do not modify these tests during the green phase — they are the contract.
//!
//! # Contract
//!
//! ```rust,ignore
//! async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError>;
//! ```
//!
//! - Returns `Some(memory)` when a memory with exactly that `id` exists.
//! - Returns `None` when no memory has that exact `id`.
//! - MUST NOT perform prefix matching — that is a caller concern.
//! - Implementations MUST use the primary-key index (SQLite: `WHERE id = ?`,
//!   InMemory: `HashMap::get`), not a full table scan.

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore, SqliteStore};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_memory(id: &str, content: &str, kind: MemoryKind) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

// ---------------------------------------------------------------------------
// Test 7a — recall_by_id returns None for a missing id, InMemoryStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_returns_none_for_missing_in_memory() {
    let store = InMemoryStore::new();

    let result = store
        .recall_by_id("does-not-exist")
        .await
        .expect("recall_by_id must not error on empty store");

    assert!(
        result.is_none(),
        "recall_by_id must return None for an id that was never recorded; got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 7b — recall_by_id returns None for a missing id, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_returns_none_for_missing_sqlite() {
    let store = SqliteStore::in_memory()
        .await
        .expect("SqliteStore::in_memory");

    let result = store
        .recall_by_id("does-not-exist")
        .await
        .expect("recall_by_id must not error on empty store");

    assert!(
        result.is_none(),
        "SqliteStore: recall_by_id must return None for an id that was never recorded; \
         got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 8a — recall_by_id returns the memory for an exact id, InMemoryStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_returns_match_for_exact_id_in_memory() {
    let mut store = InMemoryStore::new();

    let m = make_memory("exact-id-abc123", "the full content body", MemoryKind::User);
    store.record(m.clone()).await.expect("record");

    // Also record a decoy with a different id.
    store
        .record(make_memory(
            "decoy-id-xyz",
            "decoy content",
            MemoryKind::Conversation,
        ))
        .await
        .expect("record decoy");

    let result = store
        .recall_by_id("exact-id-abc123")
        .await
        .expect("recall_by_id must not error");

    let memory = result.expect("recall_by_id must return Some for an id that was recorded");

    assert_eq!(
        memory.id, "exact-id-abc123",
        "returned memory id must match exactly"
    );
    assert_eq!(
        memory.content, "the full content body",
        "returned memory content must match"
    );
    assert_eq!(
        memory.kind,
        MemoryKind::User,
        "returned memory kind must match"
    );
}

// ---------------------------------------------------------------------------
// Test 8b — recall_by_id returns the memory for an exact id, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_returns_match_for_exact_id_sqlite() {
    let mut store = SqliteStore::in_memory()
        .await
        .expect("SqliteStore::in_memory");

    let m = make_memory(
        "exact-id-abc123",
        "the full content body",
        MemoryKind::Feedback,
    );
    store.record(m.clone()).await.expect("record");

    store
        .record(make_memory(
            "decoy-id-xyz",
            "decoy content",
            MemoryKind::Project,
        ))
        .await
        .expect("record decoy");

    let result = store
        .recall_by_id("exact-id-abc123")
        .await
        .expect("recall_by_id must not error");

    let memory = result.expect("SqliteStore: recall_by_id must return Some for a recorded id");

    assert_eq!(
        memory.id, "exact-id-abc123",
        "returned memory id must match exactly"
    );
    assert_eq!(
        memory.content, "the full content body",
        "returned memory content must match"
    );
    assert_eq!(
        memory.kind,
        MemoryKind::Feedback,
        "returned memory kind must match"
    );
}

// ---------------------------------------------------------------------------
// Test 9a — recall_by_id does NOT match a prefix, InMemoryStore
//
// `recall_by_id("abc")` must NOT match an id `"abcdef..."`.
// Prefix-based lookup is a `memory_cmd` concern.  The store contract is
// exact-match only.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_does_not_match_prefix_in_memory() {
    let mut store = InMemoryStore::new();

    // Record a memory whose id starts with "abc" but is longer.
    store
        .record(make_memory(
            "abcdef-full-uuid-suffix",
            "some content",
            MemoryKind::Conversation,
        ))
        .await
        .expect("record");

    // Querying by just the prefix must return None.
    let result = store
        .recall_by_id("abc")
        .await
        .expect("recall_by_id must not error");

    assert!(
        result.is_none(),
        "recall_by_id('abc') must NOT match id 'abcdef-full-uuid-suffix'; \
         prefix matching is not a store concern; got: {result:?}"
    );

    // Sanity: exact id does return the memory.
    let exact = store
        .recall_by_id("abcdef-full-uuid-suffix")
        .await
        .expect("recall_by_id must not error on exact id");

    assert!(
        exact.is_some(),
        "recall_by_id with the exact full id must return Some"
    );
}

// ---------------------------------------------------------------------------
// Test 9b — recall_by_id does NOT match a prefix, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_by_id_does_not_match_prefix_sqlite() {
    let mut store = SqliteStore::in_memory()
        .await
        .expect("SqliteStore::in_memory");

    store
        .record(make_memory(
            "abcdef-full-uuid-suffix",
            "some content",
            MemoryKind::Reference,
        ))
        .await
        .expect("record");

    let result = store
        .recall_by_id("abc")
        .await
        .expect("recall_by_id must not error");

    assert!(
        result.is_none(),
        "SqliteStore: recall_by_id('abc') must NOT match id 'abcdef-full-uuid-suffix'; \
         store must use WHERE id = ? not LIKE or starts_with; got: {result:?}"
    );

    // Sanity: exact id does return the memory.
    let exact = store
        .recall_by_id("abcdef-full-uuid-suffix")
        .await
        .expect("recall_by_id must not error on exact id");

    assert!(
        exact.is_some(),
        "SqliteStore: recall_by_id with the exact full id must return Some"
    );
}
