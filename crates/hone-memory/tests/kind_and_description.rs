//! Red-phase tests for `MemoryKind` enum and `description` field on `Memory`.
//!
//! All tests in this file MUST FAIL to compile against `main` because:
//!   - `MemoryKind` does not exist yet.
//!   - `Memory` has no `kind` or `description` fields.
//!   - `MemoryFilter::Kind` variant does not exist yet.
//!
//! Do not modify these tests during the green phase — they are the contract.

use hone_memory::{EncryptedStore, InMemoryStore, Memory, MemoryFilter, MemoryQuery, MemoryStore};
// These two imports will fail to compile on main — that is intentional.
use hone_memory::MemoryKind;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_memory(id: &str, content: &str, kind: MemoryKind, description: Option<&str>) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: description.map(String::from),
        embedding: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — MemoryKind serde roundtrip for all six variants
// ---------------------------------------------------------------------------

#[test]
fn test_memory_kind_serde_roundtrip_all_variants() {
    let variants = [
        (MemoryKind::Conversation, "conversation"),
        (MemoryKind::User, "user"),
        (MemoryKind::Feedback, "feedback"),
        (MemoryKind::Project, "project"),
        (MemoryKind::Reference, "reference"),
        (MemoryKind::Outcome, "outcome"),
    ];

    for (kind, expected_str) in variants {
        // Serialize to JSON string (should be a lowercase bare string, no quotes wrapper).
        let serialized = serde_json::to_string(&kind)
            .unwrap_or_else(|_| panic!("serialization failed for {kind:?}"));
        // serde_json wraps strings in quotes: `"conversation"`.
        assert_eq!(
            serialized,
            format!("\"{expected_str}\""),
            "serialized form of {kind:?} should be \"{expected_str}\""
        );

        // Deserialize back and assert equality.
        let deserialized: MemoryKind = serde_json::from_str(&serialized)
            .unwrap_or_else(|_| panic!("deserialization failed for {expected_str}"));
        assert_eq!(
            deserialized, kind,
            "roundtrip failed for {kind:?}"
        );
    }
}

#[test]
fn test_memory_kind_default_is_conversation() {
    assert_eq!(MemoryKind::default(), MemoryKind::Conversation);
}

// ---------------------------------------------------------------------------
// Test 2 — Memory with kind: User round-trips through in-memory backend
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_memory_kind_user_roundtrip_in_memory() {
    let mut store = InMemoryStore::new();

    let m = make_memory("u1", "I prefer verbose type annotations", MemoryKind::User, None);
    store.record(m.clone()).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "verbose".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "u1");
    assert_eq!(
        results[0].kind,
        MemoryKind::User,
        "kind should survive in-memory roundtrip"
    );
}

#[tokio::test]
async fn test_memory_kind_user_roundtrip_sqlite() {
    use hone_memory::SqliteStore;

    let mut store = SqliteStore::in_memory().await.unwrap();

    let m = make_memory("u1", "I prefer verbose type annotations", MemoryKind::User, None);
    store.record(m).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "verbose".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "u1");
    assert_eq!(
        results[0].kind,
        MemoryKind::User,
        "kind should survive sqlite roundtrip"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — description: Some("…") round-trips through both backends
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_description_roundtrip_in_memory() {
    let mut store = InMemoryStore::new();

    let desc = "Prefers async over sync APIs";
    let m = make_memory("d1", "async preference", MemoryKind::User, Some(desc));
    store.record(m).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "async".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].description.as_deref(),
        Some(desc),
        "description should survive in-memory roundtrip"
    );
}

#[tokio::test]
async fn test_description_roundtrip_sqlite() {
    use hone_memory::SqliteStore;

    let mut store = SqliteStore::in_memory().await.unwrap();

    let desc = "Prefers async over sync APIs";
    let m = make_memory("d1", "async preference", MemoryKind::User, Some(desc));
    store.record(m).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "async".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].description.as_deref(),
        Some(desc),
        "description should survive sqlite roundtrip"
    );
}

#[tokio::test]
async fn test_description_none_roundtrip_sqlite() {
    use hone_memory::SqliteStore;

    let mut store = SqliteStore::in_memory().await.unwrap();

    let m = make_memory("d2", "no description memory", MemoryKind::Conversation, None);
    store.record(m).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "no description".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].description, None,
        "None description should survive sqlite roundtrip"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — MemoryFilter::Kind filters correctly across recall and forget
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_filter_kind_recall_in_memory() {
    let mut store = InMemoryStore::new();

    store
        .record(make_memory("c1", "conversation one", MemoryKind::Conversation, None))
        .await
        .unwrap();
    store
        .record(make_memory("u1", "user preference one", MemoryKind::User, None))
        .await
        .unwrap();
    store
        .record(make_memory("u2", "user preference two", MemoryKind::User, None))
        .await
        .unwrap();

    // MemoryFilter::Kind is the new variant — will fail to compile on main.
    let removed = store
        .forget(&MemoryFilter::Kind(MemoryKind::Conversation))
        .await
        .unwrap();

    assert_eq!(removed, 1, "one Conversation memory should be removed");

    let remaining = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(remaining.len(), 2, "two User memories should remain");
    assert!(
        remaining.iter().all(|m| m.kind == MemoryKind::User),
        "all remaining memories should be User kind"
    );
}

#[tokio::test]
async fn test_filter_kind_forget_sqlite() {
    use hone_memory::SqliteStore;

    let mut store = SqliteStore::in_memory().await.unwrap();

    store
        .record(make_memory("f1", "feedback entry", MemoryKind::Feedback, None))
        .await
        .unwrap();
    store
        .record(make_memory("o1", "outcome entry", MemoryKind::Outcome, None))
        .await
        .unwrap();
    store
        .record(make_memory("o2", "another outcome", MemoryKind::Outcome, None))
        .await
        .unwrap();

    let removed = store
        .forget(&MemoryFilter::Kind(MemoryKind::Outcome))
        .await
        .unwrap();

    assert_eq!(removed, 2, "two Outcome memories should be removed");

    let remaining = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(remaining.len(), 1, "one Feedback memory should remain");
    assert_eq!(remaining[0].id, "f1");
}

// ---------------------------------------------------------------------------
// Test 6 — EncryptedStore<S> kind/description roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_encrypted_store_kind_description_roundtrip() {
    let inner = InMemoryStore::new();
    let mut store = EncryptedStore::new(inner, "test-passphrase-pr1")
        .await
        .expect("create encrypted store");

    let desc = "Encrypted user preference";
    let m = make_memory("e1", "prefers dark theme always", MemoryKind::User, Some(desc));
    store.record(m).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "dark theme".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "should find one memory after encrypted roundtrip");
    assert_eq!(results[0].id, "e1");
    assert_eq!(
        results[0].kind,
        MemoryKind::User,
        "kind should survive encrypted roundtrip"
    );
    assert_eq!(
        results[0].description.as_deref(),
        Some(desc),
        "description should survive encrypted roundtrip"
    );
    // Verify content was actually decrypted correctly.
    assert_eq!(results[0].content, "prefers dark theme always");
}
