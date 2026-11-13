#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G8-G10 of `docs/plans/fts5-recall-spec.md` (parity and robustness): Unicode-aware
//! prompt tokens, an index keyed on memory `id` through a mapping table (memory id -> FTS rowid; immune to
//! `VACUUM` and other rowid renumbering of `memories`),
//! and scores scaled by the memory's decay weight. API: `SqliteStore::recall_fts5` and
//! `reindex_fts` as in `fts5_index.rs`; no new API.

mod fts5_support;

use std::path::Path;

use fts5_support::{corpus, mem, q, queries};
use hone_memory::{Memory, MemoryFilter, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

async fn ids_for(store: &SqliteStore, text: &str) -> Vec<String> {
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

async fn drop_triggers(pool: &sqlx::SqlitePool) {
    let triggers: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'trigger'")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    for t in triggers {
        sqlx::query(&format!("DROP TRIGGER \"{t}\"")).execute(pool).await.unwrap();
    }
}

// ---- G8: Unicode-aware tokens ------------------------------------------------------------------

#[tokio::test]
async fn test_g8_polish_words_should_be_kept_whole_and_match() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("pl", "uwaga: zażółć gęślą jaźń w nocy", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("other", "lunch menu pasta", MemoryKind::Feedback, 2)).await.unwrap();

    assert_eq!(ids_for(&store, "zażółć gęślą jaźń").await, ["pl"]);
    assert_eq!(ids_for(&store, "gęślą").await, ["pl"], "one non-ASCII word alone");
    assert_eq!(ids_for(&store, "ZAŻÓŁĆ").await, ["pl"], "case-insensitive");
}

#[tokio::test]
async fn test_g8_accented_words_should_match_with_and_without_diacritics() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("cafe-acc", "meet at the café near the station", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("cafe-plain", "the cafe closes early on sundays", MemoryKind::Feedback, 2)).await.unwrap();
    store.record(mem("zurich", "the Zürich office address changed", MemoryKind::Feedback, 3)).await.unwrap();
    store.record(mem("rich", "a rich dessert menu", MemoryKind::Feedback, 4)).await.unwrap();

    let mut cafe = ids_for(&store, "café").await;
    cafe.sort();
    assert_eq!(cafe, ["cafe-acc", "cafe-plain"], "unicode61 folds diacritics");
    assert_eq!(ids_for(&store, "Zürich").await, ["zurich"], "kept whole, not split at the umlaut");
    assert_eq!(ids_for(&store, "zurich").await, ["zurich"]);
}

#[tokio::test]
async fn test_g8_the_token_minimum_should_count_characters_not_bytes() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("meadow", "kwieciste łąk pełne maków", MemoryKind::Feedback, 1)).await.unwrap();

    assert_eq!(ids_for(&store, "łąk").await, ["meadow"], "3 characters (6 bytes) count");
    // (Since G13 the minimum is 2 characters.) One character is not usable, however many
    // bytes: `ż`, `ą` and `ł` are 2 bytes each.
    for one_char_two_bytes in ["ż", "ą", "ł"] {
        assert!(
            ids_for(&store, one_char_two_bytes).await.is_empty(),
            "{one_char_two_bytes:?} is 1 character: not usable"
        );
    }
}

#[tokio::test]
async fn test_g8_non_ascii_and_ascii_words_in_one_prompt_should_all_count() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(mem("a", "kubernetes ingress notes", MemoryKind::Feedback, 1)).await.unwrap();
    store.record(mem("b", "łódź harbour report", MemoryKind::Feedback, 2)).await.unwrap();

    let mut got = ids_for(&store, "kubernetes, łódź!").await;
    got.sort();
    assert_eq!(got, ["a", "b"]);
}

// ---- G9: the index is keyed on id (through a mapping table since review round 1) ---------------------------------------------------------------

#[tokio::test]
async fn test_g9_vacuum_and_rowid_renumbering_must_not_misalign_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
    for i in 0..12 {
        let mut m = mem(&format!("m{i:02}"), &format!("note about uniqueword{i} and shared filler"), MemoryKind::Feedback, 10 + i);
        if i % 3 == 0 {
            m.tags = vec!["doomed".to_owned()];
        }
        store.record(m).await.unwrap();
    }
    // Delete every third row (leaving gaps in the rowids), then compact the file.
    store.forget(&MemoryFilter::Tag("doomed".to_owned())).await.unwrap();
    let pool = raw_pool(&path).await;
    sqlx::query("VACUUM").execute(&pool).await.unwrap();
    pool.close().await;
    for i in 12..16 {
        store
            .record(mem(&format!("m{i:02}"), &format!("note about uniqueword{i} and shared filler"), MemoryKind::Feedback, 10 + i))
            .await
            .unwrap();
    }

    for i in (0..16).filter(|i| i % 3 != 0 || *i >= 12) {
        let got = store.recall_fts5(&q(&format!("uniqueword{i}"), 5, None), &[]).await.unwrap();
        assert_eq!(got.len(), 1, "uniqueword{i}: {:?}", got.iter().map(|s| &s.memory.id).collect::<Vec<_>>());
        assert_eq!(got[0].memory.id, format!("m{i:02}"), "uniqueword{i} returns its own memory");
        assert_eq!(got[0].memory.content, format!("note about uniqueword{i} and shared filler"));
    }
    for i in (0..12).filter(|i| i % 3 == 0) {
        assert!(ids_for(&store, &format!("uniqueword{i}")).await.is_empty(), "deleted memory {i} is gone");
    }

    // The same holds after reopening.
    drop(store);
    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
    assert_eq!(ids_for(&store, "uniqueword7").await, ["m07"]);

    // SQLite is free to renumber rowids of a table without an integer primary key. Do it
    // explicitly, behind the triggers' back: every rowid moves by 1000 and the row count and
    // the ids are unchanged, so the index must not depend on rowids at all.
    let pool = raw_pool(&path).await;
    drop_triggers(&pool).await;
    sqlx::query("UPDATE memories SET rowid = rowid + 1000").execute(&pool).await.unwrap();
    pool.close().await;
    // (No reopen: opening may touch every row and re-index it through the triggers.)
    for i in (0..16).filter(|i| i % 3 != 0 || *i >= 12) {
        let got = store.recall_fts5(&q(&format!("uniqueword{i}"), 5, None), &[]).await.unwrap();
        assert_eq!(
            got.iter().map(|s| s.memory.id.as_str()).collect::<Vec<_>>(),
            [format!("m{i:02}")],
            "after renumbering, uniqueword{i} still returns its own memory"
        );
    }
}

#[tokio::test]
async fn test_g9_an_index_whose_ids_disagree_with_memories_should_be_rebuilt_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
        store.record(mem("a", "alpacas graze on the hillside", MemoryKind::Feedback, 1)).await.unwrap();
        store.record(mem("b", "quokkas nap under the eucalyptus", MemoryKind::Feedback, 2)).await.unwrap();
    }
    // Behind the index's back: replace `b` by `c`. The row count is unchanged (2 and 2),
    // and the new row reuses the freed rowid, so only an id comparison can notice.
    {
        let pool = raw_pool(&path).await;
        drop_triggers(&pool).await;
        sqlx::query("DELETE FROM memories WHERE id = 'b'").execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, kind) \
             VALUES ('c', 'dolphins swim in the bay', '[]', 1.0, 3, 'feedback')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;
    }

    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();

    assert_eq!(ids_for(&store, "dolphins").await, ["c"], "the new row is indexed");
    assert!(ids_for(&store, "quokkas").await.is_empty(), "the removed row is not");
    assert_eq!(ids_for(&store, "alpacas").await, ["a"]);
}

// ---- G10: weight scales the score ------------------------------------------------------------------

fn weighted(id: &str, content: &str, weight: f64, created_at: u64) -> Memory {
    let mut m = mem(id, content, MemoryKind::Feedback, created_at);
    m.weight = weight;
    m
}

#[tokio::test]
async fn test_g10_a_decayed_memory_should_rank_below_an_equally_matching_fresh_one() {
    let store = SqliteStore::in_memory().await.unwrap();
    // The decayed one is NEWER, so recency cannot be what puts the fresh one first.
    store.record(weighted("fresh", "rotate the kubernetes ingress certificate", 1.0, 1)).await.unwrap();
    store.record(weighted("decayed", "rotate the kubernetes ingress certificate", 0.2, 2)).await.unwrap();

    let got = store.recall_fts5(&q("kubernetes ingress certificate", 5, None), &[]).await.unwrap();

    assert_eq!(got.iter().map(|s| s.memory.id.as_str()).collect::<Vec<_>>(), ["fresh", "decayed"]);
    let ratio = f64::from(got[1].score) / f64::from(got[0].score);
    assert!((ratio - 0.2).abs() < 1e-3, "score = -bm25 x weight: ratio {ratio}");
}

#[tokio::test]
async fn test_g10_a_strongly_decayed_strong_match_should_rank_below_a_fresh_weaker_one() {
    let store = SqliteStore::in_memory().await.unwrap();
    store.record(weighted("old-strong", "alpha alpha alpha alpha alpha", 0.05, 1)).await.unwrap();
    store.record(weighted("fresh-weak", "alpha beta gamma delta", 1.0, 2)).await.unwrap();

    let got = ids_for(&store, "alpha").await;

    assert_eq!(got, ["fresh-weak", "old-strong"]);
}

#[tokio::test]
async fn test_g10_top_results_should_match_the_rust_bm25_on_a_mixed_weight_corpus() {
    let store = SqliteStore::in_memory().await.unwrap();
    for m in corpus() {
        store.record(m).await.unwrap();
    }
    // Decayed, NEWER twins of three winners: same text, weight 0.05. Without the weight the
    // twin ties with the winner and wins on recency.
    for (topic, t) in [("k8s", 9_001u64), ("pg", 9_002), ("rust", 9_003)] {
        let win = corpus().into_iter().find(|m| m.id == format!("{topic}-win")).unwrap();
        store.record(weighted(&format!("{topic}-twin"), &win.content, 0.05, t)).await.unwrap();
    }

    for (query, winner) in queries() {
        let bm25 = store.recall(&q(query, 5, None)).await.unwrap();
        let fts = store.recall_fts5(&q(query, 5, None), &[]).await.unwrap();

        assert_eq!(bm25[0].id, winner, "fixture check: bm25 top for {query:?}");
        assert_eq!(fts[0].memory.id, bm25[0].id, "{query:?}: fts5 top equals bm25 top");
    }
}
