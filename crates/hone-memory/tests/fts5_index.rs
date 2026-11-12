#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G1 and G2 of `docs/plans/fts5-recall-spec.md`: the FTS5 index over `memories` and
//! the `fts5` recall built on it.
//!
//! API required (new, on `SqliteStore`):
//!
//! ```ignore
//! /// FTS5 recall. `query.text` is a prompt: it is turned into an FTS5 query from its
//! /// word tokens (letters, digits, `_`; >= 3 characters; lowercased; English stop words
//! /// dropped; the first 32 kept), each quoted as a string and OR-ed. No usable tokens
//! /// -> empty. `query.kind` and `exclude` are applied in the same SQL query; the top
//! /// `query.limit` by `-bm25()` are returned (score higher = better, descending), with
//! /// no corpus cap.
//! pub async fn recall_fts5(&self, query: &MemoryQuery, exclude: &[MemoryKind])
//!     -> Result<Vec<ScoredMemory>, MemoryError>;
//! /// Rebuilds the FTS5 index from `memories`; returns the number of memories indexed.
//! pub async fn reindex_fts(&self) -> Result<usize, MemoryError>;
//! ```
//!
//! The FTS5 table is named `memories_fts` (external content on `memories`); tests drop
//! it and its triggers with raw SQL to simulate an old database.

mod fts5_support;

use std::path::Path;

use fts5_support::{mem, q};
use hone_memory::{Memory, MemoryFilter, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

fn ids(v: &[hone_memory::ScoredMemory]) -> Vec<&str> {
    v.iter().map(|s| s.memory.id.as_str()).collect()
}

async fn fts(store: &SqliteStore, text: &str) -> Vec<String> {
    store
        .recall_fts5(&q(text, 10, None), &[])
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.memory.id)
        .collect()
}

async fn raw_pool(path: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display()))
        .await
        .unwrap()
}

/// Removes the FTS5 table and every trigger, as an old database would not have them.
async fn strip_fts(path: &Path) {
    let pool = raw_pool(path).await;
    let triggers: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'trigger'")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    for t in triggers {
        sqlx::query(&format!("DROP TRIGGER \"{t}\"")).execute(&pool).await.unwrap();
    }
    sqlx::query("DROP TABLE IF EXISTS memories_fts").execute(&pool).await.unwrap();
    pool.close().await;
}

async fn has_fts_table(path: &Path) -> bool {
    let pool = raw_pool(path).await;
    let n: i64 = sqlx::query("SELECT count(*) FROM sqlite_master WHERE name = 'memories_fts'")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    pool.close().await;
    n == 1
}

// ---- G1: the index mirrors the memories -----------------------------------------------------

#[tokio::test]
async fn test_g1_a_recorded_memory_should_be_found_by_its_words() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem("a", "rotate the kubernetes ingress certificate", MemoryKind::Feedback, 1))
        .await
        .unwrap();

    assert_eq!(fts(&store, "kubernetes certificate").await, ["a"]);
}

#[tokio::test]
async fn test_g1_an_insert_or_replace_update_should_reindex_the_new_content_and_drop_the_old() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("a", "alpacas graze on the hillside", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("b", "unrelated lunch menu", MemoryKind::Feedback, 2)).await.unwrap();

    // `record` is INSERT OR REPLACE: same id, new content.
    store.record(mem("a", "quokkas nap under the eucalyptus", MemoryKind::Feedback, 3)).await.unwrap();

    assert!(fts(&store, "alpacas hillside").await.is_empty(), "old words are gone");
    assert_eq!(fts(&store, "quokkas eucalyptus").await, ["a"], "exactly one hit for the id");
}

#[tokio::test]
async fn test_g1_forgotten_memories_should_leave_the_index() {
    let store = SqliteStore::in_memory().await.unwrap();
    let mut doomed = mem("a", "ephemeral zorblax note", MemoryKind::Feedback, 1);
    doomed.tags = vec!["doomed".to_owned()];
    store.record(doomed).await.unwrap();
    store.record(mem("b", "durable zorblax note", MemoryKind::Feedback, 2)).await.unwrap();

    store.forget(&MemoryFilter::Tag("doomed".to_owned())).await.unwrap();

    assert_eq!(fts(&store, "zorblax").await, ["b"]);
}

#[tokio::test]
async fn test_g1_an_old_database_without_the_fts_table_should_be_indexed_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
        store.record(mem("a", "rotate the kubernetes ingress certificate", MemoryKind::Feedback, 1)).await.unwrap();
        store.record(mem("b", "lunch menu pasta", MemoryKind::Feedback, 2)).await.unwrap();
    }
    strip_fts(&path).await;
    assert!(!has_fts_table(&path).await, "harness: the old database has no FTS table");

    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();

    assert!(has_fts_table(&path).await);
    assert_eq!(fts(&store, "kubernetes certificate").await, ["a"], "existing rows are indexed");
    // and the triggers are back: new writes are indexed too
    store.record(mem("c", "quokkas nap", MemoryKind::Feedback, 3)).await.unwrap();
    assert_eq!(fts(&store, "quokkas").await, ["c"]);
}

#[tokio::test]
async fn test_g1_a_row_count_mismatch_should_trigger_a_rebuild_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
        store.record(mem("a", "first note about alpacas", MemoryKind::Feedback, 1)).await.unwrap();
    }
    // Remove the triggers and add a row behind the index's back: memories has 2 rows, the index 1.
    {
        let pool = raw_pool(&path).await;
        let triggers: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'trigger'")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get::<String, _>(0))
            .collect();
        for t in triggers {
            sqlx::query(&format!("DROP TRIGGER \"{t}\"")).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, kind) \
             VALUES ('sneaky', 'sneaky quokkas arrived', '[]', 1.0, 5, 'feedback')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();

    assert_eq!(fts(&store, "quokkas").await, ["sneaky"]);
    assert_eq!(fts(&store, "alpacas").await, ["a"]);
}

#[tokio::test]
async fn test_g1_reindex_should_return_the_number_of_memories_indexed() {
    let store = SqliteStore::in_memory().await.unwrap();
    for i in 0..7 {
        store.record(mem(&format!("m{i}"), &format!("note number {i} about quokkas"), MemoryKind::Feedback, i)).await.unwrap();
    }

    assert_eq!(store.reindex_fts().await.unwrap(), 7);
    assert_eq!(fts(&store, "quokkas").await.len(), 7, "still searchable after a rebuild");
}

// ---- G2: recall --------------------------------------------------------------------------------

#[tokio::test]
async fn test_g2_a_relevant_memory_older_than_six_thousand_newer_ones_should_be_found() {
    let store = SqliteStore::in_memory().await.unwrap();
    store
        .record(mem("relevant", "rotate the kubernetes ingress certificate before it expires", MemoryKind::Conversation, 1))
        .await
        .unwrap();
    for i in 0..6_000u64 {
        store
            .record(mem(&format!("noise-{i}"), &format!("lunch menu number {i}: pasta and salad"), MemoryKind::Conversation, 100 + i))
            .await
            .unwrap();
    }

    let got = store.recall_fts5(&q("kubernetes ingress certificate", 5, None), &[]).await.unwrap();

    assert_eq!(ids(&got), ["relevant"], "no corpus cap");
}

#[tokio::test]
async fn test_g2_kind_filter_and_exclusions_should_apply_before_the_limit() {
    let store = SqliteStore::in_memory().await.unwrap();
    for i in 0..5 {
        store
            .record(mem(&format!("out-{i}"), "zorblax zorblax zorblax rebasing log of the zorblax branch", MemoryKind::Outcome, 10 + i))
            .await
            .unwrap();
    }
    store.record(mem("fb", "zorblax branch rebasing is preferred", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("proj", "zorblax project deadline", MemoryKind::Project, 2)).await.unwrap();

    let excluded = store.recall_fts5(&q("zorblax", 2, None), &[MemoryKind::Outcome]).await.unwrap();
    let mut got = ids(&excluded);
    got.sort_unstable();
    assert_eq!(got, ["fb", "proj"], "outcome logs must not crowd out the limit");

    let only_fb = store.recall_fts5(&q("zorblax", 5, Some(MemoryKind::Feedback)), &[]).await.unwrap();
    assert_eq!(ids(&only_fb), ["fb"]);

    let contradictory = store.recall_fts5(&q("zorblax", 5, Some(MemoryKind::Outcome)), &[MemoryKind::Outcome]).await.unwrap();
    assert!(contradictory.is_empty());
}

#[tokio::test]
async fn test_g2_fts5_syntax_in_a_prompt_should_be_treated_literally() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("k8s", "rotate the kubernetes ingress certificate", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("near", "the cabin stands near the lake", MemoryKind::Feedback, 2)).await.unwrap();
    store.record(mem("title", "title page of the manual", MemoryKind::Feedback, 3)).await.unwrap();

    // None of these may error; each is matched by its words only.
    for (prompt, expect) in [
        ("\"kubernetes ingress", "k8s"),
        ("\"kubernetes\" \"ingress\"", "k8s"),
        ("kubernetes*", "k8s"),
        ("kube* ingress certificate*", "k8s"),
        ("-certificate", "k8s"),
        ("NEAR(lake cabin", "near"),
        ("title:manual", "title"),
        ("(kubernetes AND ingress", "k8s"),
        ("ingress OR certificate)", "k8s"),
        ("kubernetes NOT ingress", "k8s"),
        ("^kubernetes {ingress} [certificate]", "k8s"),
        ("kubernetes' OR 1=1; --", "k8s"),
    ] {
        let got = store
            .recall_fts5(&q(prompt, 5, None), &[])
            .await
            .unwrap_or_else(|e| panic!("{prompt:?} must not be an FTS5 syntax error: {e}"));
        assert_eq!(got.first().map(|s| s.memory.id.as_str()), Some(expect), "{prompt:?}");
    }
    // `title:manual` is two plain words, not a column filter: a memory with only "title" matches too.
    let got = store.recall_fts5(&q("title:manual", 5, None), &[]).await.unwrap();
    assert_eq!(ids(&got), ["title"]);
    let got = store.recall_fts5(&q("title:cabin", 5, None), &[]).await.unwrap();
    assert_eq!(got.len(), 2, "either word matches: {:?}", ids(&got));
}

#[tokio::test]
async fn test_g2_a_prompt_without_usable_tokens_should_return_nothing() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("a", "the quick brown fox jumps over the lazy dog", MemoryKind::Feedback, 1)).await.unwrap();

    for prompt in ["", "   ", "a an to", "!!! ??? ***", "ab 12", "the and for with", "\"\"", "()"] {
        let got = store.recall_fts5(&q(prompt, 5, None), &[]).await.unwrap();
        assert!(got.is_empty(), "{prompt:?}: {:?}", ids(&got));
    }
}

#[tokio::test]
async fn test_g2_stop_words_should_not_make_a_memory_match() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("a", "the cat and the dog with the bird", MemoryKind::Feedback, 1)).await.unwrap();

    let got = store.recall_fts5(&q("the and with kubernetes", 5, None), &[]).await.unwrap();

    assert!(got.is_empty(), "only stop words overlap: {:?}", ids(&got));
}

#[tokio::test]
async fn test_g2_at_most_32_tokens_of_a_prompt_should_be_used_the_rarest_ones() {
    // (Since review round 1, G13: when a prompt has more than 32 distinct usable tokens, the
    // 32 RAREST are kept, not the first 32.) tok001-tok008 are common (6 documents each carry
    // them); tok009-tok040 occur in one document only.
    let store = SqliteStore::in_memory().await.unwrap();
    let common: String = (1..=8).map(|i| format!("tok{i:03} ")).collect();
    for i in 0..6 {
        store.record(mem(&format!("common-{i}"), &format!("{common}filler {i}"), MemoryKind::Feedback, 1 + i)).await.unwrap();
    }
    let rare: String = (9..=40).map(|i| format!("tok{i:03} ")).collect();
    store.record(mem("rare32", &rare, MemoryKind::Feedback, 10)).await.unwrap();
    let prompt: String = (1..=40).map(|i| format!("tok{i:03} ")).collect();

    let got = store.recall_fts5(&q(&prompt, 10, None), &[]).await.unwrap();

    assert_eq!(ids(&got), ["rare32"], "the 8 common tokens are the ones cut, so the common documents do not match");
}

#[tokio::test]
async fn test_g2_scores_should_be_positive_descending_and_limited() {
    let store = SqliteStore::in_memory().await.unwrap();
    for i in 0..10u64 {
        let content = format!("{} filler words here", "alpha ".repeat(10 - i as usize));
        store.record(mem(&format!("m{i}"), &content, MemoryKind::Conversation, 10 + i)).await.unwrap();
    }

    let got = store.recall_fts5(&q("alpha", 4, None), &[]).await.unwrap();

    assert_eq!(got.len(), 4);
    assert!(got.iter().all(|s| s.score > 0.0), "{:?}", got.iter().map(|s| s.score).collect::<Vec<_>>());
    assert!(got.windows(2).all(|w| w[0].score >= w[1].score), "descending");
    assert_eq!(ids(&got)[0], "m0", "the most relevant first, not the newest");
}

#[tokio::test]
async fn test_g2_results_should_be_whole_memories() {
    let store = SqliteStore::in_memory().await.unwrap();
    let mut m: Memory = mem("full", "kubernetes ingress notes", MemoryKind::Reference, 42);
    m.tags = vec!["infra".to_owned()];
    m.description = Some("about ingress".to_owned());
    m.weight = 0.5;
    store.record(m.clone()).await.unwrap();

    let got = store.recall_fts5(&q("kubernetes", 5, None), &[]).await.unwrap();

    assert_eq!(got[0].memory, m);
}
