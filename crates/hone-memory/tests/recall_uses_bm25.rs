use hone_memory::{InMemoryStore, Memory, MemoryQuery, MemoryStore, SqliteStore};

fn make_memory(id: &str, content: &str, weight: f64) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        weight,
        created_at: 1_000_000,
        decay_at: None,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Test 11 — SqliteStore: BM25 beats substring-count ordering
// ---------------------------------------------------------------------------

/// A substring counter would rank a doc that repeats a common word many times
/// over a doc that contains one rare term once.  BM25 IDF must reverse that.
#[tokio::test]
async fn sqlite_recall_orders_by_bm25_not_substring_count() {
    // memory A: common word "the" repeated many times → substring counter favours A.
    // memory B: rare term "quuxbaz" appears once           → BM25 IDF favours B.
    // memory C: irrelevant content.
    //
    // Query: "quuxbaz"
    // Expected order with BM25: B first (and A must NOT appear, since it doesn't
    // contain "quuxbaz").  We add a third memory that contains "quuxbaz" + lots
    // of filler so that length normalisation is exercised and B (short) beats C.
    let tmp = tempfile::NamedTempFile::new().unwrap();

    let mut store = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .unwrap();

    // memory_a: many occurrences of a common word but not the query term.
    let memory_a = make_memory(
        "mem_a",
        "the the the the the the the the the the",
        1.0,
    );
    // memory_b: the rare query term once, short doc → should outscore mem_c.
    let memory_b = make_memory("mem_b", "quuxbaz systems", 1.0);
    // memory_c: the rare query term once but surrounded by lots of filler.
    let filler = "word ".repeat(30);
    let content_c = format!("quuxbaz {filler}");
    let memory_c = make_memory("mem_c", &content_c, 1.0);

    store.record(memory_a).await.unwrap();
    store.record(memory_b).await.unwrap();
    store.record(memory_c).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "quuxbaz".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    // mem_a should not appear (it doesn't contain "quuxbaz").
    assert!(
        results.iter().all(|m| m.id != "mem_a"),
        "mem_a (common-word doc) should not appear in 'quuxbaz' results"
    );

    // Among results that do contain "quuxbaz", the shorter doc (mem_b) must rank first.
    assert!(
        !results.is_empty(),
        "at least one result must match 'quuxbaz'"
    );
    assert_eq!(
        results[0].id, "mem_b",
        "BM25 length normalisation must rank the shorter 'quuxbaz' doc first"
    );
}

// ---------------------------------------------------------------------------
// Test 12 — Weight multiplier is preserved alongside BM25
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_score_still_combines_with_weight_and_decay() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .unwrap();

    // Identical content — only weight differs.
    let low_weight = make_memory("low", "rust ownership lifetime", 1.0);
    let high_weight = make_memory("high", "rust ownership lifetime", 10.0);

    store.record(low_weight).await.unwrap();
    store.record(high_weight).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "rust".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].id, "high",
        "higher-weight memory should rank first when content is identical"
    );
    assert_eq!(results[1].id, "low");
}

// ---------------------------------------------------------------------------
// Test 13 — Empty query returns all memories (existing contract preserved)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_empty_query_still_returns_all_memories() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut store = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .unwrap();

    store
        .record(make_memory("x", "totally unrelated alpha", 1.0))
        .await
        .unwrap();
    store
        .record(make_memory("y", "totally unrelated beta", 1.0))
        .await
        .unwrap();
    store
        .record(make_memory("z", "totally unrelated gamma", 1.0))
        .await
        .unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 3, "empty query must return all memories");
}

// ---------------------------------------------------------------------------
// Test 14 — InMemoryStore also uses BM25 (not the old substring counter)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn in_memory_recall_orders_by_bm25_too() {
    let mut store = InMemoryStore::new();

    // Same scenario as test 11: short doc with rare term beats long doc.
    let memory_b = make_memory("mem_b", "quuxbaz systems", 1.0);
    let filler = "word ".repeat(30);
    let content_c = format!("quuxbaz {filler}");
    let memory_c = make_memory("mem_c", &content_c, 1.0);
    // Irrelevant doc that the old substring counter could accidentally score.
    let memory_a = make_memory(
        "mem_a",
        "the the the the the the the the the the",
        1.0,
    );

    store.record(memory_a).await.unwrap();
    store.record(memory_b).await.unwrap();
    store.record(memory_c).await.unwrap();

    let results = store
        .recall(&MemoryQuery {
            text: "quuxbaz".to_string(),
            limit: 10,
        })
        .await
        .unwrap();

    assert!(
        results.iter().all(|m| m.id != "mem_a"),
        "mem_a should not appear in 'quuxbaz' results (InMemoryStore)"
    );
    assert!(
        !results.is_empty(),
        "at least one result must match 'quuxbaz'"
    );
    assert_eq!(
        results[0].id, "mem_b",
        "InMemoryStore BM25 must rank the shorter 'quuxbaz' doc first"
    );
}
