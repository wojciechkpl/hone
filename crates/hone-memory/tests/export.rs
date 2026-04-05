use hone_memory::{InMemoryStore, Memory, MemoryQuery, MemoryStore};

fn make_memory(id: &str, content: &str) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec!["export-test".to_string()],
        weight: 1.0,
        created_at: 42_000,
        decay_at: None,
    }
}

#[tokio::test]
async fn test_export_produces_valid_json() {
    let mut store = InMemoryStore::new();

    let m1 = make_memory("export-1", "first memory for export");
    let m2 = make_memory("export-2", "second memory for export");

    store.record(m1.clone()).await.unwrap();
    store.record(m2.clone()).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memories.json");

    store.export(&path).await.unwrap();

    let imported = InMemoryStore::import(&path).await.unwrap();

    let query = MemoryQuery {
        text: String::new(),
        limit: 100,
    };
    let mut results = imported.recall(&query).await.unwrap();
    assert_eq!(results.len(), 2, "both memories should survive the round-trip");

    results.sort_by(|a, b| a.id.cmp(&b.id));
    let mut expected = vec![m1, m2];
    expected.sort_by(|a, b| a.id.cmp(&b.id));

    for (got, want) in results.iter().zip(expected.iter()) {
        assert_eq!(got, want);
    }
}
