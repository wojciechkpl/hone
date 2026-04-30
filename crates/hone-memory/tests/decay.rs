use hone_memory::{InMemoryStore, Memory, MemoryQuery, MemoryStore};

fn make_memory(id: &str, content: &str, weight: f64, decay_at: Option<u64>) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight,
        created_at: 1_000_000,
        decay_at,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_decay_reduces_weight() {
    let mut store = InMemoryStore::new();

    let m = make_memory("m1", "important context", 1.0, None);
    let id = m.id.clone();
    store.record(m).await.unwrap();

    store.decay(0.5, 0).await.unwrap();

    let query = MemoryQuery {
        text: "important".to_string(),
        limit: 10,
    };
    let results = store.recall(&query).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, id);
    assert!(
        (results[0].weight - 0.5).abs() < f64::EPSILON,
        "expected weight 0.5, got {}",
        results[0].weight
    );
}

#[tokio::test]
async fn test_decay_removes_expired_memories() {
    let mut store = InMemoryStore::new();

    let expired = make_memory("expired", "old context", 1.0, Some(100));
    let alive = make_memory("alive", "fresh context", 1.0, Some(300));

    store.record(expired).await.unwrap();
    store.record(alive.clone()).await.unwrap();

    let removed = store.decay(1.0, 200).await.unwrap();
    assert_eq!(removed, 1, "one expired memory should have been removed");

    let query = MemoryQuery {
        text: String::new(),
        limit: 100,
    };
    let remaining = store.recall(&query).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, alive.id);
}
