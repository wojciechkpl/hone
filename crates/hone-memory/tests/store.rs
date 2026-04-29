use hone_memory::{InMemoryStore, Memory, MemoryFilter, MemoryQuery, MemoryStore};

fn make_memory(id: &str, content: &str, tags: Vec<&str>, weight: f64) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: tags.into_iter().map(String::from).collect(),
        weight,
        created_at: 1_000_000,
        decay_at: None,
    }
}

#[tokio::test]
async fn test_record_and_recall_basic() {
    let mut store = InMemoryStore::new();

    let m = make_memory("m1", "the quick brown fox", vec![], 1.0);
    store.record(m.clone()).await.unwrap();

    let query = MemoryQuery {
        text: "quick".to_string(),
        limit: 10,
    };
    let results = store.recall(&query).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, m.id);
    assert_eq!(results[0].content, m.content);
}

#[tokio::test]
async fn test_recall_respects_relevance_ordering() {
    let mut store = InMemoryStore::new();

    let low = make_memory("low", "rust programming language", vec![], 1.0);
    let high = make_memory("high", "rust is great for systems programming", vec![], 5.0);
    let mid = make_memory("mid", "programming in rust is fun", vec![], 3.0);

    store.record(low.clone()).await.unwrap();
    store.record(high.clone()).await.unwrap();
    store.record(mid.clone()).await.unwrap();

    let query = MemoryQuery {
        text: "rust".to_string(),
        limit: 10,
    };
    let results = store.recall(&query).await.unwrap();

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].id, high.id, "highest weight should be first");
    assert_eq!(results[1].id, mid.id, "mid weight should be second");
    assert_eq!(results[2].id, low.id, "lowest weight should be last");
}

#[tokio::test]
async fn test_forget_by_tag_removes_matching() {
    let mut store = InMemoryStore::new();

    let keep = make_memory("keep", "unrelated memory", vec!["project:bar"], 1.0);
    let remove1 = make_memory("rm1", "first foo memory", vec!["project:foo"], 1.0);
    let remove2 = make_memory(
        "rm2",
        "second foo memory",
        vec!["project:foo", "extra"],
        1.0,
    );

    store.record(keep.clone()).await.unwrap();
    store.record(remove1).await.unwrap();
    store.record(remove2).await.unwrap();

    let removed = store
        .forget(&MemoryFilter::Tag("project:foo".to_string()))
        .await
        .unwrap();

    assert_eq!(removed, 2);

    let query = MemoryQuery {
        text: String::new(),
        limit: 100,
    };
    let remaining = store.recall(&query).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, keep.id);
}
