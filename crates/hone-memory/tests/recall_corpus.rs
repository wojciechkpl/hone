#![allow(clippy::unwrap_used, clippy::expect_used)]
//! M1-M3 of `docs/plans/memory-recall-index-spec.md`: `SqliteStore::recall`
//! ranks the whole corpus (the newest `MAX_RECALL_CORPUS` rows after any kind
//! filter) and only then takes the top `limit`; outcome logs can be excluded in
//! SQL before the cap.
//!
//! API required (new):
//!
//! ```ignore
//! // MemoryStore trait, with a DEFAULT implementation so that existing impls and
//! // mocks keep compiling (the default may recall and then drop the excluded
//! // kinds). `SqliteStore` MUST override it so the exclusion happens in SQL
//! // before the corpus cap, and `EncryptedStore` must delegate. `MemoryQuery`
//! // is unchanged, so no existing `MemoryQuery { .. }` literal needs an edit.
//! async fn recall_excluding(
//!     &self,
//!     query: &MemoryQuery,
//!     exclude: &[MemoryKind],
//! ) -> Result<Vec<Memory>, MemoryError>;
//! ```
//!
//! `recall_excluding(q, &[])` behaves exactly like `recall(q)`. With `q.kind =
//! Some(k)` and `k` excluded the result is empty.

use hone_memory::{Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};

fn mem(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

fn q(text: &str, limit: usize, kind: Option<MemoryKind>) -> MemoryQuery {
    MemoryQuery {
        text: text.to_owned(),
        limit,
        kind,
    }
}

fn ids(v: &[Memory]) -> Vec<&str> {
    v.iter().map(|m| m.id.as_str()).collect()
}

/// One relevant memory (the OLDEST), then `newer` unrelated ones.
async fn store_with_old_relevant(newer: u64) -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem(
            "relevant",
            "rotate the kubernetes ingress certificate before it expires",
            MemoryKind::Conversation,
            1,
        ))
        .await
        .unwrap();
    for i in 0..newer {
        store
            .record(mem(
                &format!("noise-{i}"),
                &format!("lunch menu number {i}: pasta and salad"),
                MemoryKind::Conversation,
                100 + i,
            ))
            .await
            .unwrap();
    }
    store
}

// ---- M1 ------------------------------------------------------------------------

#[tokio::test]
async fn test_m1_relevant_memory_older_than_the_newest_limit_should_be_found() {
    let store = store_with_old_relevant(5).await;
    let got = store
        .recall(&q("kubernetes ingress certificate", 5, None))
        .await
        .unwrap();
    assert_eq!(ids(&got), ["relevant"]);
}

#[tokio::test]
async fn test_m1_relevant_memory_beyond_the_newest_fifty_should_be_found() {
    let store = store_with_old_relevant(60).await;
    let got = store
        .recall(&q("kubernetes ingress certificate", 5, None))
        .await
        .unwrap();
    assert_eq!(ids(&got), ["relevant"]);
}

#[tokio::test]
async fn test_m1_top_limit_should_be_chosen_by_relevance_not_by_recency() {
    let store = SqliteStore::in_memory().await.unwrap();
    // Ten memories mention "alpha" a different number of times. The most
    // relevant ones are the OLDEST.
    for i in 0..10u64 {
        let repeats = 10 - i as usize; // id 0 is oldest and most relevant
        let content = format!("{} filler words here", "alpha ".repeat(repeats));
        store
            .record(mem(&format!("m{i}"), &content, MemoryKind::Conversation, 10 + i))
            .await
            .unwrap();
    }
    let got = store.recall(&q("alpha", 3, None)).await.unwrap();
    let mut got_ids = ids(&got);
    got_ids.sort_unstable();
    assert_eq!(got_ids, ["m0", "m1", "m2"], "the best three, not the newest three");
}

#[tokio::test]
async fn test_m1_memory_without_a_lexical_match_should_still_not_be_returned() {
    let store = store_with_old_relevant(20).await;
    let got = store.recall(&q("zebra xylophone", 5, None)).await.unwrap();
    assert!(got.is_empty(), "{:?}", ids(&got));
}

#[tokio::test]
async fn test_m1_empty_query_should_still_enumerate_the_newest_limit() {
    let store = SqliteStore::in_memory().await.unwrap();
    for i in 0..10u64 {
        store
            .record(mem(&format!("m{i}"), "anything", MemoryKind::Conversation, 10 + i))
            .await
            .unwrap();
    }
    let got = store.recall(&q("", 3, None)).await.unwrap();
    let mut got_ids = ids(&got);
    got_ids.sort_unstable();
    assert_eq!(got_ids, ["m7", "m8", "m9"], "unchanged: the newest, no ranking");
}

// ---- M2 ------------------------------------------------------------------------

#[tokio::test]
async fn test_m2_kind_filter_should_apply_before_the_cap_and_ranking_covers_the_kind() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem(
            "relevant-feedback",
            "always run the migration dry run before deploying",
            MemoryKind::Feedback,
            1,
        ))
        .await
        .unwrap();
    for i in 0..10u64 {
        store
            .record(mem(
                &format!("feedback-noise-{i}"),
                &format!("prefer short commit messages {i}"),
                MemoryKind::Feedback,
                100 + i,
            ))
            .await
            .unwrap();
        // Newer rows of another kind that also match the text.
        store
            .record(mem(
                &format!("conversation-{i}"),
                "migration dry run deploying",
                MemoryKind::Conversation,
                200 + i,
            ))
            .await
            .unwrap();
    }
    let got = store
        .recall(&q("migration dry run deploying", 5, Some(MemoryKind::Feedback)))
        .await
        .unwrap();
    assert_eq!(ids(&got), ["relevant-feedback"]);
}

// ---- M3 ------------------------------------------------------------------------

/// A strongly matching `Outcome` log (newest) and a weaker matching older
/// `Feedback`; five outcomes so that a post-hoc filter of the top 5 would be empty.
async fn store_with_outcomes() -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem(
            "feedback",
            "deploy the widget only after the smoke test passes",
            MemoryKind::Feedback,
            1,
        ))
        .await
        .unwrap();
    for i in 0..6u64 {
        store
            .record(mem(
                &format!("outcome-{i}"),
                "deploy widget deploy widget deploy widget smoke test",
                MemoryKind::Outcome,
                100 + i,
            ))
            .await
            .unwrap();
    }
    store
}

#[tokio::test]
async fn test_m3_excluded_kind_should_be_left_out_before_the_cap() {
    let store = store_with_outcomes().await;
    let got = store
        .recall_excluding(&q("deploy widget smoke test", 5, None), &[MemoryKind::Outcome])
        .await
        .unwrap();
    assert_eq!(ids(&got), ["feedback"], "the outcomes outrank it, but are excluded in SQL");
}

#[tokio::test]
async fn test_m3_without_exclusion_outcomes_should_still_be_recalled() {
    let store = store_with_outcomes().await;
    let plain = store
        .recall(&q("deploy widget smoke test", 5, None))
        .await
        .unwrap();
    assert!(plain.iter().all(|m| m.kind == MemoryKind::Outcome), "{:?}", ids(&plain));
    assert_eq!(plain.len(), 5);

    let none_excluded = store
        .recall_excluding(&q("deploy widget smoke test", 5, None), &[])
        .await
        .unwrap();
    assert_eq!(ids(&none_excluded), ids(&plain), "an empty exclusion list changes nothing");
}

#[tokio::test]
async fn test_m3_exclusion_should_combine_with_a_kind_filter() {
    let store = store_with_outcomes().await;
    let got = store
        .recall_excluding(
            &q("deploy widget", 5, Some(MemoryKind::Outcome)),
            &[MemoryKind::Outcome],
        )
        .await
        .unwrap();
    assert!(got.is_empty());
    let got = store
        .recall_excluding(
            &q("deploy widget", 5, Some(MemoryKind::Feedback)),
            &[MemoryKind::Outcome],
        )
        .await
        .unwrap();
    assert_eq!(ids(&got), ["feedback"]);
}

#[tokio::test]
async fn test_m3_exclusion_should_apply_to_enumeration_too() {
    let store = store_with_outcomes().await;
    let got = store
        .recall_excluding(&q("", 10, None), &[MemoryKind::Outcome])
        .await
        .unwrap();
    assert_eq!(ids(&got), ["feedback"]);
}

// ---- M10: listing keeps the embedding -------------------------------------------
//
// The BM25 corpus query may leave `embedding` out (M8), but enumeration (an empty
// query, used by listings, the memory index and `GET /memory`) must still return
// it. For a non-empty query either is acceptable and not pinned.

fn with_embedding(mut m: Memory, e: Vec<f32>) -> Memory {
    m.embedding = Some(e);
    m
}

#[tokio::test]
async fn test_m10_empty_query_enumeration_should_return_the_stored_embedding() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(with_embedding(
            mem("with", "has a vector", MemoryKind::Feedback, 2),
            vec![0.25, -0.5, 1.0],
        ))
        .await
        .unwrap();
    store
        .record(mem("without", "no vector", MemoryKind::Feedback, 1))
        .await
        .unwrap();

    let got = store.recall(&q("", 10, None)).await.unwrap();

    let with = got.iter().find(|m| m.id == "with").unwrap();
    assert_eq!(with.embedding.as_deref(), Some(&[0.25, -0.5, 1.0][..]));
    let without = got.iter().find(|m| m.id == "without").unwrap();
    assert!(without.embedding.is_none());
}

#[tokio::test]
async fn test_m10_enumeration_with_a_kind_filter_should_return_the_embedding_too() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(with_embedding(
            mem("with", "has a vector", MemoryKind::User, 1),
            vec![1.0, 2.0],
        ))
        .await
        .unwrap();

    let got = store.recall(&q("", 10, Some(MemoryKind::User))).await.unwrap();

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].embedding.as_deref(), Some(&[1.0, 2.0][..]));
}

#[tokio::test]
async fn test_m10_enumeration_with_exclusions_should_return_the_embedding_too() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(with_embedding(
            mem("with", "has a vector", MemoryKind::Feedback, 1),
            vec![3.0],
        ))
        .await
        .unwrap();

    let got = store
        .recall_excluding(&q("", 10, None), &[MemoryKind::Outcome])
        .await
        .unwrap();

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].embedding.as_deref(), Some(&[3.0][..]));
}

// ---- M11: a ranked (searched) recall returns the embedding too ---------------------
//
// Supersedes the M10 note that a non-empty query may omit it: `GET /memory?q=`
// (the memory panel's search) reports `has_embedding` from what `recall` returns.

async fn store_with_one_vector() -> SqliteStore {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(with_embedding(
            mem("with", "the quokka enclosure needs repainting", MemoryKind::Feedback, 2),
            vec![0.25, -0.5, 1.0],
        ))
        .await
        .unwrap();
    store
        .record(mem("without", "the quokka feeding schedule", MemoryKind::Feedback, 1))
        .await
        .unwrap();
    store
}

#[tokio::test]
async fn test_m11_ranked_recall_should_return_the_stored_embedding() {
    let store = store_with_one_vector().await;

    let got = store.recall(&q("quokka", 10, None)).await.unwrap();

    assert_eq!(got.len(), 2);
    let with = got.iter().find(|m| m.id == "with").unwrap();
    assert_eq!(with.embedding.as_deref(), Some(&[0.25, -0.5, 1.0][..]));
    let without = got.iter().find(|m| m.id == "without").unwrap();
    assert!(without.embedding.is_none(), "no vector stored, none returned");
}

#[tokio::test]
async fn test_m11_ranked_recall_with_a_kind_filter_should_return_the_embedding() {
    let store = store_with_one_vector().await;

    let got = store
        .recall(&q("quokka", 10, Some(MemoryKind::Feedback)))
        .await
        .unwrap();

    let with = got.iter().find(|m| m.id == "with").unwrap();
    assert_eq!(with.embedding.as_deref(), Some(&[0.25, -0.5, 1.0][..]));
}

#[tokio::test]
async fn test_m11_ranked_recall_excluding_should_return_the_embedding() {
    let store = store_with_one_vector().await;

    let got = store
        .recall_excluding(&q("quokka", 10, None), &[MemoryKind::Outcome])
        .await
        .unwrap();

    let with = got.iter().find(|m| m.id == "with").unwrap();
    assert_eq!(with.embedding.as_deref(), Some(&[0.25, -0.5, 1.0][..]));
    let without = got.iter().find(|m| m.id == "without").unwrap();
    assert!(without.embedding.is_none());
}
