use hone_memory::{Memory, MemoryFilter, MemoryQuery, MemoryStore, SqliteStore};

fn make_memory(id: &str, content: &str, tags: Vec<&str>, weight: f64) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: tags.into_iter().map(|t| t.to_string()).collect(),
        weight,
        created_at: 1_000_000,
        decay_at: None,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_sqlite_record_and_recall() {
    let mut store = SqliteStore::in_memory().await.unwrap();

    let memory = make_memory("m1", "Rust ownership model", vec!["lang:rust"], 1.0);
    store.record(memory).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "ownership".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "m1");
    assert_eq!(results[0].content, "Rust ownership model");
}

#[tokio::test]
async fn test_sqlite_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");

    // First session: record a memory and drop the store.
    {
        let mut store = SqliteStore::open(&db_path).await.unwrap();
        let memory = make_memory("m1", "persisted memory", vec![], 1.0);
        store.record(memory).await.unwrap();
        // store drops here, closing the pool
    }

    // Second session: reopen the same file and verify the memory survived.
    {
        let store = SqliteStore::open(&db_path).await.unwrap();
        let results = store
            .recall(&MemoryQuery {
                text: "persisted".to_string(),
                limit: 10,
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1, "memory should survive a reopen");
        assert_eq!(results[0].id, "m1");
    }
}

#[tokio::test]
async fn test_sqlite_forget_by_tag() {
    let mut store = SqliteStore::in_memory().await.unwrap();

    store
        .record(make_memory("m1", "alpha", vec!["project:foo"], 1.0))
        .await
        .unwrap();
    store
        .record(make_memory("m2", "beta", vec!["project:foo", "other"], 1.0))
        .await
        .unwrap();
    store
        .record(make_memory("m3", "gamma", vec!["project:bar"], 1.0))
        .await
        .unwrap();

    let removed = store
        .forget(&MemoryFilter::Tag("project:foo".to_string()))
        .await
        .unwrap();

    assert_eq!(
        removed, 2,
        "two memories tagged project:foo should be removed"
    );

    // The remaining memory should be m3.
    let all = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "m3");
}

#[tokio::test]
async fn test_sqlite_decay_reduces_weight() {
    let mut store = SqliteStore::in_memory().await.unwrap();

    store
        .record(make_memory("m1", "decay target", vec![], 1.0))
        .await
        .unwrap();

    store.decay(0.5, 9_999_999).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "decay target".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    let weight = results[0].weight;
    assert!(
        (weight - 0.5).abs() < 1e-9,
        "weight should be ~0.5 after decay, got {weight}"
    );
}

#[tokio::test]
async fn test_sqlite_decay_removes_expired() {
    let mut store = SqliteStore::in_memory().await.unwrap();

    let memory = Memory {
        id: "m1".to_string(),
        content: "ephemeral".to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: Some(100),
        ..Default::default()
    };
    store.record(memory).await.unwrap();

    // now=200 is past the decay_at of 100, so this memory should be removed.
    let removed = store.decay(1.0, 200).await.unwrap();
    assert_eq!(removed, 1);

    let results = store
        .recall(&MemoryQuery {
            text: "ephemeral".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    assert!(
        results.is_empty(),
        "expired memory should have been deleted"
    );
}

#[tokio::test]
async fn test_sqlite_export_import_roundtrip() {
    let export_file = tempfile::NamedTempFile::new().unwrap();

    // Build a store with two memories and export it.
    let mut source = SqliteStore::in_memory().await.unwrap();
    source
        .record(make_memory("m1", "first memory", vec!["tag:a"], 1.0))
        .await
        .unwrap();
    source
        .record(make_memory("m2", "second memory", vec!["tag:b"], 0.8))
        .await
        .unwrap();
    source.export(export_file.path()).await.unwrap();

    // Import into a fresh in-memory store.
    let imported = SqliteStore::import(export_file.path()).await.unwrap();

    let all = imported
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(all.len(), 2, "both memories should survive export/import");

    let ids: std::collections::HashSet<&str> = all.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains("m1"));
    assert!(ids.contains("m2"));
}
