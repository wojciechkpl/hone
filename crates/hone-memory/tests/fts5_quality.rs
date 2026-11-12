#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G3 of `docs/plans/fts5-recall-spec.md`: on a fixed corpus (36 memories, 10 queries),
//! the `fts5` recall's top result equals the in-Rust BM25 recall's (`SqliteStore::recall`)
//! top result for every query where both return something, and `fts5` returns only
//! memories that share at least one token with the query. API: see `fts5_index.rs`.

mod fts5_support;

use fts5_support::{corpus, queries, q, word_tokens};
use hone_memory::{MemoryStore, SqliteStore};

async fn loaded() -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    for m in corpus() {
        store.record(m).await.unwrap();
    }
    store
}

#[tokio::test]
async fn test_g3_the_top_result_should_match_the_rust_bm25_recall_for_every_query() {
    let store = loaded().await;

    for (query, winner) in queries() {
        let bm25 = store.recall(&q(query, 5, None)).await.unwrap();
        let fts = store.recall_fts5(&q(query, 5, None), &[]).await.unwrap();

        assert!(!bm25.is_empty() && !fts.is_empty(), "{query:?}: both must find something");
        assert_eq!(bm25[0].id, winner, "fixture check: bm25 top for {query:?}");
        assert_eq!(fts[0].memory.id, bm25[0].id, "{query:?}: fts5 top equals bm25 top");
    }
}

#[tokio::test]
async fn test_g3_fts5_should_return_only_memories_sharing_a_token_with_the_query() {
    let store = loaded().await;

    for (query, _) in queries() {
        let wanted = word_tokens(query);
        let fts = store.recall_fts5(&q(query, 10, None), &[]).await.unwrap();
        for s in &fts {
            assert!(
                !word_tokens(&s.memory.content).is_disjoint(&wanted),
                "{query:?}: {} shares no token with the query",
                s.memory.id
            );
        }
    }
}
