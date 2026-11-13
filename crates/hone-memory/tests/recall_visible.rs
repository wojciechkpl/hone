#![allow(clippy::unwrap_used, clippy::expect_used)]
//! W22 of `docs/plans/memory-write-tool-spec.md` (review round 3): project visibility is
//! filtered in the store, before the corpus cap.
//!
//! API (new), a `MemoryStore` trait method with a default implementation (recall, then
//! filter) so other stores keep compiling:
//!
//! ```ignore
//! async fn recall_visible(&self, query: &MemoryQuery, exclude: &[MemoryKind],
//!                         project: Option<&str>) -> Result<Vec<Memory>, MemoryError>;
//! ```
//!
//! Semantics: like `recall_excluding(query, exclude)`, and additionally a memory tagged
//! `source:tool` is returned only when it carries the tag `project:<id>` for the given project
//! `<id>` (`project: Some(id)`). Memories without the `source:tool` tag are never affected.
//! `SqliteStore` does this in SQL BEFORE its 5,000-row corpus window; `EncryptedStore` on its
//! plaintext tags.

use hone_memory::{
    EncryptedStore, InMemoryStore, Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore,
};

fn mem(id: &str, content: &str, kind: MemoryKind, tags: &[&str], created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

fn q(text: &str, limit: usize) -> MemoryQuery {
    MemoryQuery {
        text: text.to_owned(),
        limit,
        kind: None,
    }
}

fn ids(v: &[Memory]) -> Vec<String> {
    let mut ids: Vec<String> = v.iter().map(|m| m.id.clone()).collect();
    ids.sort();
    ids
}

const WORDS: &str = "zorblax deploy checklist";

/// The oldest rows: B's tool memory, an untagged memory, a user memory tagged with another
/// project (not a tool memory), a tool memory of A, all matching the query; then `newer`
/// newer tool memories of project A.
async fn fill(store: &dyn MemoryStore, newer: u64) {
    store
        .record(mem("b-tool", &format!("{WORDS} for project b"), MemoryKind::Reference, &["source:tool", "project:bbb"], 1))
        .await
        .unwrap();
    store
        .record(mem("untagged", &format!("{WORDS} written by the user"), MemoryKind::Reference, &[], 2))
        .await
        .unwrap();
    store
        .record(mem("user-with-project-tag", &format!("{WORDS} user note"), MemoryKind::Reference, &["project:aaa"], 3))
        .await
        .unwrap();
    for i in 0..newer {
        store
            .record(mem(
                &format!("a-tool-{i}"),
                &format!("{WORDS} number {i} for project a"),
                MemoryKind::Reference,
                &["source:tool", "project:aaa"],
                100 + i,
            ))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn test_w22_a_sqlite_store_should_find_a_project_memory_older_than_six_thousand_other_project_tool_memories() {
    let store = SqliteStore::in_memory().await.unwrap();
    fill(&store, 6_000).await;

    let found = store
        .recall_visible(&q(WORDS, 50), &[MemoryKind::Outcome], Some("bbb"))
        .await
        .unwrap();

    let got = ids(&found);
    assert!(got.contains(&"b-tool".to_owned()), "the project's own old memory is found: {} results", got.len());
    assert!(got.contains(&"untagged".to_owned()), "non-tool memories stay visible");
    assert!(got.contains(&"user-with-project-tag".to_owned()), "only source:tool rows are project-scoped");
    assert!(
        found.iter().all(|m| !m.id.starts_with("a-tool-")),
        "another project's tool memories are not visible"
    );
}

#[tokio::test]
async fn test_w22_exclude_kinds_should_still_apply() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem("outcome", &format!("{WORDS} outcome log"), MemoryKind::Outcome, &[], 1))
        .await
        .unwrap();
    store
        .record(mem("note", &format!("{WORDS} note"), MemoryKind::Reference, &[], 2))
        .await
        .unwrap();

    let found = store
        .recall_visible(&q(WORDS, 10), &[MemoryKind::Outcome], Some("bbb"))
        .await
        .unwrap();

    assert_eq!(ids(&found), ["note"]);
}

#[tokio::test]
async fn test_w22_the_default_implementation_should_recall_then_filter() {
    let store = InMemoryStore::new();
    fill(&store, 20).await;

    let found = store
        .recall_visible(&q(WORDS, 100), &[], Some("bbb"))
        .await
        .unwrap();

    let got = ids(&found);
    assert!(got.contains(&"b-tool".to_owned()), "{got:?}");
    assert!(got.contains(&"untagged".to_owned()));
    assert!(got.iter().all(|i| !i.starts_with("a-tool-")), "{got:?}");

    let from_a = store
        .recall_visible(&q(WORDS, 100), &[], Some("aaa"))
        .await
        .unwrap();
    assert!(!ids(&from_a).contains(&"b-tool".to_owned()));
    assert_eq!(from_a.iter().filter(|m| m.id.starts_with("a-tool-")).count(), 20);
}

#[tokio::test]
async fn test_w22_an_encrypted_store_should_filter_on_its_plaintext_tags_before_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let inner = SqliteStore::open_with_decay_interval(&dir.path().join("memory.db"), 0)
        .await
        .unwrap();
    let store = EncryptedStore::new(inner, "passphrase").await.unwrap();
    fill(&store, 6_000).await;

    let found = store
        .recall_visible(&q(WORDS, 50), &[MemoryKind::Outcome], Some("bbb"))
        .await
        .unwrap();

    let got = ids(&found);
    assert!(got.contains(&"b-tool".to_owned()), "{} results", got.len());
    assert!(got.contains(&"untagged".to_owned()));
    assert!(found.iter().all(|m| !m.id.starts_with("a-tool-")));
}
