#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G11, G12, G14 and G16 of `docs/plans/fts5-recall-spec.md` (review round 1): FTS5 writes
//! cost the same at any store size, opening a store never fails when another opens it at the
//! same time, a reindex is atomic, and forgotten text leaves the index. No new API
//! (`SqliteStore::open_with_decay_interval`, `record`, `forget`, `reindex_fts`, `recall_fts5`).
//!
//! Timing: tests run in the `test` profile, which this workspace builds optimised (the
//! spec's "release" bounds therefore apply). Bounds are generous multiples of the spec's:
//! 5 ms (the spec's bound) for the MEDIAN of 40 single-memory writes at 50,000 memories, and
//! 3 s (spec 1 s) for a decay pass over 10,000. Measured on the unfixed code: 8-44 ms per
//! write and 25-58 s for the decay.
//! The big stores are built with raw SQL (triggers dropped, one transaction) and one
//! `reindex_fts`, because seeding 50,000 rows through `record` on the unfixed code would
//! itself take minutes.

mod fts5_support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fts5_support::{mem, q};
use hone_memory::{MemoryFilter, MemoryKind, MemoryStore, SqliteStore};
use sqlx::Row;

async fn raw_pool(path: &Path) -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", path.display()))
        .await
        .unwrap()
}

async fn drop_triggers(pool: &sqlx::SqlitePool) {
    let names: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'trigger'")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    for t in names {
        sqlx::query(&format!("DROP TRIGGER \"{t}\"")).execute(pool).await.unwrap();
    }
}

/// `n` memories `m0..` ("note <i> word<i> filler text"), indexed.
async fn seeded(path: &Path, n: usize) -> SqliteStore {
    let store = SqliteStore::open_with_decay_interval(path, NEVER).await.unwrap();
    let pool = raw_pool(path).await;
    drop_triggers(&pool).await;
    sqlx::query(&format!(
        "WITH RECURSIVE c(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM c WHERE i < {})
         INSERT INTO memories (id, content, tags, weight, created_at, decay_at, kind)
         SELECT 'm' || i, 'note ' || i || ' word' || i || ' filler text', '[]', 1.0, i, NULL, 'feedback' FROM c",
        n - 1
    ))
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    assert_eq!(store.reindex_fts().await.unwrap(), n);
    store
}

/// A decay interval that never comes due.
const NEVER: u64 = 3_153_600_000;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

// ---- G11 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g11_single_writes_at_fifty_thousand_memories_should_stay_fast() {
    let dir = tempfile::tempdir().unwrap();
    let store = seeded(&dir.path().join("memory.db"), 50_000).await;

    let (mut insert, mut replace, mut forget) = (vec![], vec![], vec![]);
    for i in 0..40usize {
        let mut fresh = mem(&format!("new{i}"), &format!("fresh note {i} about quokkas"), MemoryKind::Feedback, 100_000 + i as u64);
        fresh.tags = vec![format!("t{i}")];
        let t = Instant::now();
        store.record(fresh).await.unwrap();
        insert.push(t.elapsed());

        let t = Instant::now();
        store.record(mem(&format!("m{}", i * 7), &format!("replaced {i} text about alpacas"), MemoryKind::Feedback, 5)).await.unwrap();
        replace.push(t.elapsed());

        let t = Instant::now();
        store.forget(&MemoryFilter::Tag(format!("t{i}"))).await.unwrap();
        forget.push(t.elapsed());
    }

    let bound = Duration::from_millis(5);
    let (mi, mr, mf) = (median(insert), median(replace), median(forget));
    assert!(mi < bound, "median record of a new memory {mi:?} (bound {bound:?})");
    assert!(mr < bound, "median replace {mr:?} (bound {bound:?})");
    assert!(mf < bound, "median forget {mf:?} (bound {bound:?})");
    // and the index still answers
    assert_eq!(store.recall_fts5(&q("quokkas", 3, None), &[]).await.unwrap().len(), 0, "forgotten");
    assert_eq!(store.recall_fts5(&q("alpacas", 100, None), &[]).await.unwrap().len(), 40);
}

#[tokio::test]
async fn test_g11_the_daily_decay_on_open_should_finish_in_seconds_at_ten_thousand_memories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(seeded(&path, 10_000).await);

    let t = Instant::now();
    // Interval 0: the decay is due on every open (it rewrites every weight).
    let store = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
    let took = t.elapsed();

    assert!(took < Duration::from_secs(3), "open with decay due took {took:?}");
    assert_eq!(store.recall_fts5(&q("word4242", 3, None), &[]).await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_g11_a_weight_only_update_should_not_touch_the_fts_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = SqliteStore::open_with_decay_interval(&path, NEVER).await.unwrap();
    for i in 0..5 {
        store.record(mem(&format!("m{i}"), &format!("note {i} about quokkas"), MemoryKind::Feedback, i)).await.unwrap();
    }
    let pool = raw_pool(&path).await;
    let mut conn = pool.acquire().await.unwrap();

    let before: i64 = sqlx::query_scalar("SELECT total_changes()").fetch_one(&mut *conn).await.unwrap();
    sqlx::query("UPDATE memories SET weight = 0.5 WHERE id = 'm2'").execute(&mut *conn).await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT total_changes()").fetch_one(&mut *conn).await.unwrap();

    // `total_changes()` counts rows changed by triggers too: just the one memories row.
    assert_eq!(after - before, 1, "a weight update must not delete and re-insert index rows");

    // A content update still reindexes.
    sqlx::query("UPDATE memories SET content = 'now about alpacas' WHERE id = 'm2'").execute(&mut *conn).await.unwrap();
    drop(conn);
    pool.close().await;
    let got: Vec<String> = store.recall_fts5(&q("alpacas", 5, None), &[]).await.unwrap().into_iter().map(|s| s.memory.id).collect();
    assert_eq!(got, ["m2"]);
}

// ---- G12 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g12_concurrent_opens_of_a_fresh_database_should_all_succeed() {
    for i in 0..100 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");

        let (a, b) = tokio::join!(
            SqliteStore::open_with_decay_interval(&path, 0),
            SqliteStore::open_with_decay_interval(&path, 0)
        );

        a.unwrap_or_else(|e| panic!("iteration {i}: first open failed: {e}"));
        b.unwrap_or_else(|e| panic!("iteration {i}: second open failed: {e}"));
    }
}

#[tokio::test]
async fn test_g12_concurrent_opens_of_an_existing_database_should_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    {
        let s = SqliteStore::open_with_decay_interval(&path, 0).await.unwrap();
        s.record(mem("a", "alpacas graze", MemoryKind::Feedback, 1)).await.unwrap();
    }
    for i in 0..100 {
        let (a, b, c) = tokio::join!(
            SqliteStore::open_with_decay_interval(&path, 0),
            SqliteStore::open_with_decay_interval(&path, 0),
            SqliteStore::open_with_decay_interval(&path, 0)
        );
        for (n, r) in [a, b, c].into_iter().enumerate() {
            r.unwrap_or_else(|e| panic!("iteration {i}, open {n} failed: {e}"));
        }
    }
}

#[tokio::test]
async fn test_g12_triggers_should_be_left_alone_unless_their_sql_differs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    drop(SqliteStore::open_with_decay_interval(&path, NEVER).await.unwrap());

    // A normal open does not drop and re-create them: same rowids and SQL afterwards.
    let snapshot = || async {
        let pool = raw_pool(&path).await;
        let rows: Vec<(i64, String, String)> = sqlx::query("SELECT rowid, name, sql FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect();
        pool.close().await;
        rows
    };
    let before = snapshot().await;
    assert!(!before.is_empty(), "harness: the FTS triggers exist");
    drop(SqliteStore::open_with_decay_interval(&path, NEVER).await.unwrap());
    assert_eq!(snapshot().await, before, "nothing dropped and re-created");

    // One whose stored SQL is outdated is replaced.
    let pool = raw_pool(&path).await;
    let names: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE '%_ad'")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    assert_eq!(names.len(), 1, "harness: one delete trigger: {names:?}");
    // An outdated definition: it forgets to delete from the index.
    sqlx::query(&format!("DROP TRIGGER \"{}\"", names[0])).execute(&pool).await.unwrap();
    sqlx::query(&format!("CREATE TRIGGER \"{}\" AFTER DELETE ON memories BEGIN SELECT 1; END", names[0])).execute(&pool).await.unwrap();
    pool.close().await;

    let store = SqliteStore::open_with_decay_interval(&path, NEVER).await.unwrap();
    store.record(mem("a", "zorblax note", MemoryKind::Feedback, 1)).await.unwrap();
    let mut doomed = mem("b", "zorblax other", MemoryKind::Feedback, 2);
    doomed.tags = vec!["x".to_owned()];
    store.record(doomed).await.unwrap();
    store.forget(&MemoryFilter::Tag("x".to_owned())).await.unwrap();

    let got: Vec<String> = store.recall_fts5(&q("zorblax", 5, None), &[]).await.unwrap().into_iter().map(|s| s.memory.id).collect();
    assert_eq!(got, ["a"], "the current trigger definition was restored");
}

// ---- G14 ---------------------------------------------------------------------------------------

#[tokio::test]
async fn test_g14_a_reindex_should_never_show_a_concurrent_recall_an_empty_or_doubled_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(seeded(&dir.path().join("memory.db"), 3_000).await);
    let expected = 3_000usize;

    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for _ in 0..6 {
                store.reindex_fts().await.unwrap();
            }
        })
    };
    let mut observations = 0;
    while !writer.is_finished() {
        let got = store.recall_fts5(&q("filler", expected + 100, None), &[]).await.unwrap();
        let mut ids: Vec<&str> = got.iter().map(|s| s.memory.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total, "a doubled index returned duplicates");
        assert_eq!(total, expected, "a recall during a reindex saw {total} of {expected} memories");
        observations += 1;
    }
    writer.await.unwrap();
    assert!(observations >= 3, "harness: the recalls overlapped the reindex ({observations})");
}

// ---- G16 ---------------------------------------------------------------------------------------

async fn index_data_contains(path: &Path, needle: &[u8]) -> bool {
    let pool = raw_pool(path).await;
    let blobs: Vec<Vec<u8>> = sqlx::query("SELECT block FROM memories_fts_data")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<Vec<u8>, _>(0))
        .collect();
    pool.close().await;
    blobs.iter().any(|b| b.windows(needle.len()).any(|w| w == needle))
}

#[tokio::test]
async fn test_g16_a_forgotten_memorys_tokens_should_not_remain_in_the_fts_shadow_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.db");
    let store = SqliteStore::open_with_decay_interval(&path, NEVER).await.unwrap();
    let mut secret = mem("secret", "the vault code is quokkasecretword today", MemoryKind::Feedback, 1);
    secret.tags = vec!["private".to_owned()];
    store.record(secret).await.unwrap();
    store.record(mem("other", "ordinary note about alpacas", MemoryKind::Feedback, 2)).await.unwrap();
    assert!(index_data_contains(&path, b"quokkasecretword").await, "harness: the token is in the index data before the forget");

    store.forget(&MemoryFilter::Tag("private".to_owned())).await.unwrap();

    assert!(!index_data_contains(&path, b"quokkasecretword").await, "the forgotten token is gone from the index data");
    assert!(index_data_contains(&path, b"alpacas").await, "other memories stay indexed");
}
