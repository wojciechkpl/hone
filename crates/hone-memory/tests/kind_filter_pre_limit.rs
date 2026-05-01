//! Red-phase tests for `MemoryQuery::kind` — the pre-limit kind filter.
//!
//! These tests will FAIL TO COMPILE on `main` because:
//!   - `MemoryQuery` has no `kind` field (currently `{ text: String, limit: usize }`)
//!   - `MemoryQuery` does not implement `Default`
//!
//! Do not modify these tests during the green phase — they are the contract.
//!
//! # Bug being caught
//!
//! `bins/hone/src/memory_cmd.rs` currently does:
//!   1. `recall(&MemoryQuery { text: "", limit: N })`
//!   2. `memories.retain(|m| m.kind == target)`
//!
//! Step 2 is post-limit: if the store returns `N` non-User memories from a
//! corpus of 53 (50 Conversation + 3 User), the retain leaves 0 results even
//! though 3 User memories exist.  The fix is to filter BEFORE applying limit
//! inside the store.
//!
//! The tests below prove that the fix is correct by constructing exactly the
//! adversarial ordering that exposes the bug.

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

fn make_memory_with_kind(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — kind filter is applied BEFORE limit, InMemoryStore
//
// Scenario: 50 Conversation memories inserted first (lower created_at),
// then 3 User memories inserted last (higher created_at → sorted first by
// recency/weight).  With a buggy post-limit retain the 3 User memories ARE
// returned — but then we also verify the inverse ordering, where the bug
// produces 0.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_pre_limit_in_memory_store() {
    let mut store = InMemoryStore::new();

    // Insert 50 Conversation memories with created_at 1..=50.
    for i in 1..=50u64 {
        store
            .record(make_memory_with_kind(
                &format!("conv-{i:03}"),
                &format!("conversation memory number {i}"),
                MemoryKind::Conversation,
                i,
            ))
            .await
            .expect("record conversation");
    }

    // Insert 3 User memories AFTER the conversations (higher created_at).
    for i in 1..=3u64 {
        store
            .record(make_memory_with_kind(
                &format!("user-{i}"),
                &format!("user preference number {i}"),
                MemoryKind::User,
                100 + i,
            ))
            .await
            .expect("record user");
    }

    // With kind filter applied PRE-limit, recall(kind=User, limit=10) must
    // return all 3 User memories regardless of how many Conversation memories
    // exist in the store.
    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall with kind filter");

    assert_eq!(
        results.len(),
        3,
        "recall with kind=User limit=10 must return all 3 User memories; \
         a post-limit retain would return 0 because the 10 most-recent include only \
         the 3 User memories — but insert the conversations LAST to expose the real bug"
    );
    assert!(
        results.iter().all(|m| m.kind == MemoryKind::User),
        "every returned memory must be kind=User"
    );

    // --- Inverse ordering: User first, then 50 Conversations (higher created_at).
    // With a buggy post-limit retain and limit=10, recall returns 10 Conversation
    // memories, then retain leaves 0.  With a correct pre-limit filter, returns 3.
    let mut store2 = InMemoryStore::new();

    for i in 1..=3u64 {
        store2
            .record(make_memory_with_kind(
                &format!("user-inv-{i}"),
                &format!("user preference inverse {i}"),
                MemoryKind::User,
                i,
            ))
            .await
            .expect("record user inverse");
    }
    for i in 1..=50u64 {
        store2
            .record(make_memory_with_kind(
                &format!("conv-inv-{i:03}"),
                &format!("conversation inverse {i}"),
                MemoryKind::Conversation,
                100 + i,
            ))
            .await
            .expect("record conversation inverse");
    }

    let results2 = store2
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall inverse with kind filter");

    assert_eq!(
        results2.len(),
        3,
        "inverse ordering: recall with kind=User limit=10 must still return all 3 User \
         memories even when 50 Conversation memories have higher created_at; \
         a post-limit retain leaves 0 (the 10 retrieved are all Conversation)"
    );
    assert!(
        results2.iter().all(|m| m.kind == MemoryKind::User),
        "every returned memory must be kind=User (inverse ordering)"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — kind filter is applied BEFORE limit, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_pre_limit_sqlite_store() {
    let mut store = SqliteStore::in_memory().await.expect("SqliteStore::in_memory");

    // 50 Conversation memories, created_at 1..=50.
    for i in 1..=50u64 {
        store
            .record(make_memory_with_kind(
                &format!("conv-{i:03}"),
                &format!("conversation memory number {i}"),
                MemoryKind::Conversation,
                i,
            ))
            .await
            .expect("record conversation");
    }

    // 3 User memories, created_at 101..=103 (sorted first by DESC created_at in SQL).
    for i in 1..=3u64 {
        store
            .record(make_memory_with_kind(
                &format!("user-{i}"),
                &format!("user preference number {i}"),
                MemoryKind::User,
                100 + i,
            ))
            .await
            .expect("record user");
    }

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall with kind filter");

    assert_eq!(
        results.len(),
        3,
        "SqliteStore: recall kind=User limit=10 must return all 3 User memories; \
         a LIMIT applied before the WHERE kind='user' would return 10 Conversation rows"
    );
    assert!(
        results.iter().all(|m| m.kind == MemoryKind::User),
        "every returned memory must be kind=User"
    );

    // --- Inverse: User inserted first (lower created_at), Conversations inserted last.
    let mut store2 = SqliteStore::in_memory().await.expect("SqliteStore::in_memory");

    for i in 1..=3u64 {
        store2
            .record(make_memory_with_kind(
                &format!("user-inv-{i}"),
                &format!("user preference inverse {i}"),
                MemoryKind::User,
                i,
            ))
            .await
            .expect("record user inverse");
    }
    for i in 1..=50u64 {
        store2
            .record(make_memory_with_kind(
                &format!("conv-inv-{i:03}"),
                &format!("conversation inverse {i}"),
                MemoryKind::Conversation,
                100 + i,
            ))
            .await
            .expect("record conversation inverse");
    }

    let results2 = store2
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall inverse with kind filter");

    assert_eq!(
        results2.len(),
        3,
        "SqliteStore inverse ordering: recall kind=User limit=10 must return all 3 User \
         memories even when the 10 most-recent by created_at are all Conversation"
    );
    assert!(
        results2.iter().all(|m| m.kind == MemoryKind::User),
        "every returned memory must be kind=User (inverse ordering)"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — kind: None returns memories of all kinds, InMemoryStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_none_returns_all_kinds_in_memory() {
    let mut store = InMemoryStore::new();

    store
        .record(make_memory_with_kind(
            "c1",
            "conversation",
            MemoryKind::Conversation,
            1,
        ))
        .await
        .expect("record c1");
    store
        .record(make_memory_with_kind("u1", "user pref", MemoryKind::User, 2))
        .await
        .expect("record u1");
    store
        .record(make_memory_with_kind(
            "f1",
            "feedback rule",
            MemoryKind::Feedback,
            3,
        ))
        .await
        .expect("record f1");
    store
        .record(make_memory_with_kind(
            "p1",
            "project fact",
            MemoryKind::Project,
            4,
        ))
        .await
        .expect("record p1");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall kind=None");

    assert_eq!(
        results.len(),
        4,
        "kind=None must return all 4 memories across all kinds"
    );

    let kinds: std::collections::HashSet<_> = results.iter().map(|m| m.kind).collect();
    assert!(
        kinds.contains(&MemoryKind::Conversation),
        "result must include Conversation"
    );
    assert!(
        kinds.contains(&MemoryKind::User),
        "result must include User"
    );
    assert!(
        kinds.contains(&MemoryKind::Feedback),
        "result must include Feedback"
    );
    assert!(
        kinds.contains(&MemoryKind::Project),
        "result must include Project"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — kind: None returns memories of all kinds, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_none_returns_all_kinds_sqlite() {
    let mut store = SqliteStore::in_memory().await.expect("SqliteStore::in_memory");

    store
        .record(make_memory_with_kind(
            "c1",
            "conversation",
            MemoryKind::Conversation,
            1,
        ))
        .await
        .expect("record c1");
    store
        .record(make_memory_with_kind("u1", "user pref", MemoryKind::User, 2))
        .await
        .expect("record u1");
    store
        .record(make_memory_with_kind(
            "f1",
            "feedback rule",
            MemoryKind::Feedback,
            3,
        ))
        .await
        .expect("record f1");
    store
        .record(make_memory_with_kind(
            "o1",
            "outcome record",
            MemoryKind::Outcome,
            4,
        ))
        .await
        .expect("record o1");

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .expect("recall kind=None");

    assert_eq!(
        results.len(),
        4,
        "SqliteStore kind=None must return all 4 memories across all kinds"
    );

    let kinds: std::collections::HashSet<_> = results.iter().map(|m| m.kind).collect();
    assert!(
        kinds.contains(&MemoryKind::Conversation),
        "result must include Conversation"
    );
    assert!(
        kinds.contains(&MemoryKind::User),
        "result must include User"
    );
    assert!(
        kinds.contains(&MemoryKind::Feedback),
        "result must include Feedback"
    );
    assert!(
        kinds.contains(&MemoryKind::Outcome),
        "result must include Outcome"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — kind filter composes with text/BM25 query, InMemoryStore
//
// Insert memories where matching content appears in multiple kinds.
// Assert that kind=Some(K) returns only K-kind matches even when the text
// query scores across several kinds.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_with_text_query_in_memory() {
    let mut store = InMemoryStore::new();

    // "rust" appears in all three kinds.
    store
        .record(make_memory_with_kind(
            "c1",
            "rust programming in conversations",
            MemoryKind::Conversation,
            1,
        ))
        .await
        .expect("record c1");
    store
        .record(make_memory_with_kind(
            "u1",
            "user prefers rust over C++",
            MemoryKind::User,
            2,
        ))
        .await
        .expect("record u1");
    store
        .record(make_memory_with_kind(
            "u2",
            "always use rust for systems code",
            MemoryKind::User,
            3,
        ))
        .await
        .expect("record u2");
    store
        .record(make_memory_with_kind(
            "f1",
            "rust clippy must be clean — feedback rule",
            MemoryKind::Feedback,
            4,
        ))
        .await
        .expect("record f1");

    // Text query "rust" would normally match all four.
    // With kind=Some(User), only the two User memories must be returned.
    let results = store
        .recall(&MemoryQuery {
            text: "rust".to_string(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall text+kind");

    assert_eq!(
        results.len(),
        2,
        "text query 'rust' with kind=User must return exactly 2 User memories, \
         not the Conversation or Feedback matches"
    );
    assert!(
        results.iter().all(|m| m.kind == MemoryKind::User),
        "all results must be kind=User"
    );

    let ids: std::collections::HashSet<&str> = results.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains("u1"), "u1 must be in results");
    assert!(ids.contains("u2"), "u2 must be in results");
}

// ---------------------------------------------------------------------------
// Test 6 — kind filter composes with text/BM25 query, SqliteStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kind_filter_with_text_query_sqlite() {
    let mut store = SqliteStore::in_memory().await.expect("SqliteStore::in_memory");

    store
        .record(make_memory_with_kind(
            "c1",
            "rust programming in conversations",
            MemoryKind::Conversation,
            1,
        ))
        .await
        .expect("record c1");
    store
        .record(make_memory_with_kind(
            "u1",
            "user prefers rust over C++",
            MemoryKind::User,
            2,
        ))
        .await
        .expect("record u1");
    store
        .record(make_memory_with_kind(
            "u2",
            "always use rust for systems code",
            MemoryKind::User,
            3,
        ))
        .await
        .expect("record u2");
    store
        .record(make_memory_with_kind(
            "f1",
            "rust clippy must be clean — feedback rule",
            MemoryKind::Feedback,
            4,
        ))
        .await
        .expect("record f1");

    let results = store
        .recall(&MemoryQuery {
            text: "rust".to_string(),
            limit: 10,
            kind: Some(MemoryKind::User),
        })
        .await
        .expect("recall text+kind");

    assert_eq!(
        results.len(),
        2,
        "SqliteStore: text query 'rust' with kind=User must return exactly 2 User memories"
    );
    assert!(
        results.iter().all(|m| m.kind == MemoryKind::User),
        "all results must be kind=User"
    );

    let ids: std::collections::HashSet<&str> = results.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains("u1"), "u1 must be in results");
    assert!(ids.contains("u2"), "u2 must be in results");
}
