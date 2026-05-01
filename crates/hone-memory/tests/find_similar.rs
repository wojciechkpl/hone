//! Red-phase tests for R6: `find_similar` on the `MemoryStore` trait.
//!
//! These tests MUST fail to compile on `main` because `MemoryStore::find_similar`
//! does not exist yet. All three backends are exercised.
//!
//! Do not modify these tests during the green phase — they are the contract.

use hone_memory::{EncryptedStore, InMemoryStore, Memory, MemoryQuery, MemoryStore};

// ---------------------------------------------------------------------------
// Helper: construct a minimal Memory with unique id and content.
// ---------------------------------------------------------------------------

fn mem(id: &str, content: &str) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Test 15 — empty store returns None
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_similar_returns_none_when_empty() {
    let store = InMemoryStore::new();

    let result = store
        .find_similar("use rg not grep", 0.8)
        .await
        .expect("find_similar should not error on empty store");

    assert!(
        result.is_none(),
        "empty store must return None; got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 16 — highly similar content is found above threshold (InMemoryStore)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_similar_returns_match_above_threshold() {
    let mut store = InMemoryStore::new();

    store
        .record(mem("m1", "use rg, not grep, in this repo"))
        .await
        .unwrap();

    // Query is nearly identical — whitespace-normalised variant.
    let result = store
        .find_similar("use rg not grep in this repo", 0.8)
        .await
        .expect("find_similar should not error");

    assert!(
        result.is_some(),
        "near-identical query should match above 0.8 threshold; got None"
    );
    let matched = result.unwrap();
    assert_eq!(
        matched.id, "m1",
        "should have matched m1; got id: {}",
        matched.id
    );
}

// ---------------------------------------------------------------------------
// Test 17 — unrelated content is not matched above threshold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_similar_returns_none_below_threshold() {
    let mut store = InMemoryStore::new();

    store
        .record(mem("m1", "use rg, not grep, in this repo"))
        .await
        .unwrap();

    let result = store
        .find_similar("completely unrelated content about cats", 0.8)
        .await
        .expect("find_similar should not error");

    assert!(
        result.is_none(),
        "unrelated query must not match above 0.8 threshold; got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 18 — picks the highest-scoring match when multiple exist
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_similar_picks_highest_score() {
    let mut store = InMemoryStore::new();

    // Insert two memories; one is closer to the query than the other.
    store
        .record(mem("close", "use rg not grep for fast search"))
        .await
        .unwrap();
    store
        .record(mem(
            "far",
            "prefer ripgrep over grep when searching in the codebase",
        ))
        .await
        .unwrap();

    // Query is very close to "close".
    let result = store
        .find_similar("use rg not grep for fast search", 0.5)
        .await
        .expect("find_similar should not error");

    assert!(result.is_some(), "should have found a match");
    let matched = result.unwrap();
    assert_eq!(
        matched.id, "close",
        "should have matched the closer record 'close', got: {}",
        matched.id
    );
}

// ---------------------------------------------------------------------------
// Test 19 — find_similar works for InMemoryStore (canonical backend)
// ---------------------------------------------------------------------------

/// Duplicate of the above-threshold test but explicitly tagged to confirm
/// `InMemoryStore` inherits or implements `find_similar` correctly.
#[tokio::test]
async fn find_similar_works_for_in_memory_backend() {
    let mut store = InMemoryStore::new();

    store
        .record(mem("rule1", "use rg, not grep, in this repo"))
        .await
        .unwrap();

    let result = store
        .find_similar("use rg not grep in this repo", 0.8)
        .await
        .expect("InMemoryStore::find_similar should not error");

    assert!(
        result.is_some(),
        "InMemoryStore must find the similar record above 0.8 threshold"
    );
}

// ---------------------------------------------------------------------------
// Test 20 — find_similar works for EncryptedStore<InMemoryStore>
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_similar_works_for_encrypted_backend() {
    let inner = InMemoryStore::new();
    let mut store = EncryptedStore::new(inner, "test-passphrase")
        .await
        .expect("create EncryptedStore");

    store
        .record(mem("rule1", "use rg, not grep, in this repo"))
        .await
        .unwrap();

    let result = store
        .find_similar("use rg not grep in this repo", 0.8)
        .await
        .expect("EncryptedStore::find_similar should not error");

    assert!(
        result.is_some(),
        "EncryptedStore must find the similar record above 0.8 threshold"
    );
}

// ---------------------------------------------------------------------------
// Test 21 — recall("") returns all records up to limit (contract pin)
//
// The default `find_similar` implementation calls `recall` with an empty text
// query to fetch the full store, then scores in-memory.  If any backend
// interprets empty text as "return nothing", `find_similar` is silently
// broken for every query.  This test pins that contract for each backend.
//
// This test SHOULD PASS today.  It is a regression guard so a future
// implementer cannot accidentally break the empty-query semantics.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_empty_query_returns_all_records() {
    const LIMIT: usize = 32;

    // --- InMemoryStore ---
    {
        let mut store = InMemoryStore::new();
        store.record(mem("a1", "alpha content")).await.unwrap();
        store.record(mem("a2", "beta content")).await.unwrap();
        store.record(mem("a3", "gamma content")).await.unwrap();

        let results = store
            .recall(&MemoryQuery {
                text: "".into(),
                limit: LIMIT,
                kind: None,
            })
            .await
            .expect("InMemoryStore::recall should not error with empty text");

        assert_eq!(
            results.len(),
            3,
            "InMemoryStore: recall with empty text must return all 3 records \
             (got {}); if this fails, the backend treats empty text as 'no match' \
             which silently breaks find_similar for every query",
            results.len()
        );
    }

    // --- EncryptedStore<InMemoryStore> ---
    {
        let inner = InMemoryStore::new();
        let mut store = EncryptedStore::new(inner, "test-passphrase")
            .await
            .expect("create EncryptedStore");
        store.record(mem("b1", "alpha content")).await.unwrap();
        store.record(mem("b2", "beta content")).await.unwrap();
        store.record(mem("b3", "gamma content")).await.unwrap();

        let results = store
            .recall(&MemoryQuery {
                text: "".into(),
                limit: LIMIT,
                kind: None,
            })
            .await
            .expect("EncryptedStore::recall should not error with empty text");

        assert_eq!(
            results.len(),
            3,
            "EncryptedStore: recall with empty text must return all 3 records \
             (got {}); if this fails, the backend treats empty text as 'no match' \
             which silently breaks find_similar for every query",
            results.len()
        );
    }
}
