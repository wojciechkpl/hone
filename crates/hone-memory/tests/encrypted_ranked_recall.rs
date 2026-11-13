#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G5 of `docs/plans/fts5-recall-spec.md`: `EncryptedStore::recall` with a non-empty
//! query decrypts the newest memories (up to 5,000, after the kind filter and the
//! exclusions) and ranks them with the same BM25 as `SqliteStore`, returning the top
//! `limit`. Today it keeps only memories containing the WHOLE query as a substring.
//! No new API.

mod fts5_support;

use fts5_support::{corpus, mem, q, queries};
use hone_memory::{EncryptedStore, MemoryKind, MemoryStore, SqliteStore};

async fn encrypted() -> EncryptedStore<SqliteStore> {
    EncryptedStore::new(SqliteStore::in_memory().await.unwrap(), "correct horse battery")
        .await
        .unwrap()
}

#[tokio::test]
async fn test_g5_a_prompt_sharing_words_with_an_encrypted_memory_should_recall_it() {
    let store = encrypted().await;
    store.record(mem("a", "staging database password rotation runbook", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("b", "lunch menu pasta and salad", MemoryKind::Feedback, 2)).await.unwrap();

    let got = store.recall(&q("how do I rotate the staging database runbook?", 5, None)).await.unwrap();

    assert_eq!(got.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a"]);
    assert_eq!(got[0].content, "staging database password rotation runbook", "decrypted");

    // A prompt sharing no word recalls nothing; an empty query still lists, without the
    // key-salt record (an implementation detail).
    assert!(store.recall(&q("quokkas eucalyptus", 5, None)).await.unwrap().is_empty());
    let mut listed: Vec<String> = store.recall(&q("", 10, None)).await.unwrap().into_iter().map(|m| m.id).collect();
    listed.sort();
    assert_eq!(listed, ["a", "b"]);
}

#[tokio::test]
async fn test_g5_results_should_be_ranked_and_limited_like_the_sqlite_store() {
    let plain = SqliteStore::in_memory().await.unwrap();
    let enc = encrypted().await;
    for m in corpus() {
        plain.record(m.clone()).await.unwrap();
        enc.record(m).await.unwrap();
    }

    for (query, winner) in queries() {
        let want: Vec<String> = plain.recall(&q(query, 3, None)).await.unwrap().into_iter().map(|m| m.id).collect();
        let got: Vec<String> = enc.recall(&q(query, 3, None)).await.unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(got, want, "{query:?}");
        assert_eq!(got[0], winner);
    }
}

#[tokio::test]
async fn test_g5_exclusions_and_kind_should_apply_before_the_corpus_cap() {
    let store = encrypted().await;
    store.record(mem("old-feedback", "zorblax branch rebasing is preferred", MemoryKind::Feedback, 1)).await.unwrap();
    for i in 0..5_010u64 {
        store
            .record(mem(&format!("out-{i}"), "zorblax branch rebasing log entry", MemoryKind::Outcome, 100 + i))
            .await
            .unwrap();
    }

    let got = store.recall_excluding(&q("which zorblax branch rebasing do we prefer", 5, None), &[MemoryKind::Outcome]).await.unwrap();
    assert_eq!(got.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["old-feedback"]);

    let only = store.recall(&q("which zorblax branch rebasing do we prefer", 5, Some(MemoryKind::Feedback))).await.unwrap();
    assert_eq!(only.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["old-feedback"]);
}
