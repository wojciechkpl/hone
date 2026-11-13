#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R12, R14 and R16 of `docs/plans/doc-retrieval-spec.md` (review round 1) on the document
//! index, with real directories and real `git init` repositories.
//!
//! Contract pinned:
//! - R12: `sync(working_dir)` honours `.gitignore` files of the PARENT directories up to the
//!   repository root (a directory holding `.git`), `.git/info/exclude` and `.honeignore` files
//!   of those directories, so running from `repo/sub` indexes nothing `repo` excludes. (The
//!   user's global git excludes are not covered here: they depend on the machine's git config.)
//! - R14: two `DocIndex::open(db)` + `sync(dir)` running at the same time on one database file
//!   both succeed (every sync's writes are in a `BEGIN IMMEDIATE` transaction).
//! - R16: every sync records its root in a new table `doc_roots(root TEXT PRIMARY KEY,
//!   synced_at INTEGER NOT NULL)` (canonical working directory, unix seconds); on `open`,
//!   roots with `synced_at` more than 30 days ago are removed together with their files and
//!   chunks.

use std::path::Path;
use std::process::Command;

use hone_index::{DocIndex, Retriever};
use hone_proto::SourceRef;
use sqlx::Row;

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

async fn ids(idx: &DocIndex, query: &str) -> Vec<String> {
    let mut v: Vec<String> = idx
        .retriever()
        .retrieve(query, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|h| match h.source {
            SourceRef::Chunk { chunk_id, .. } => chunk_id,
            other => panic!("{other:?}"),
        })
        .collect();
    v.sort();
    v
}

// ---- R12 -----------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r12_running_from_a_subdirectory_should_honour_the_repositorys_ignore_files() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(tmp.path()).unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
    write(&repo, ".gitignore", "private/\n*.draft.md\n");
    write(&repo, ".honeignore", "internal/\n");
    write(&repo, ".git/info/exclude", "scratch/\n");
    write(&repo, "top.md", "# Top\nzebra stripes\n");
    write(&repo, "sub/keep.md", "# Keep\nzebra stripes\n");
    write(&repo, "sub/private/x.md", "# X\nzebra stripes\n");
    write(&repo, "sub/notes.draft.md", "# Draft\nzebra stripes\n");
    write(&repo, "sub/scratch/s.md", "# S\nzebra stripes\n");
    write(&repo, "sub/internal/i.md", "# I\nzebra stripes\n");
    write(&repo, "sub/deep/also.md", "# Also\nzebra stripes\n");
    let idx = DocIndex::in_memory().await.unwrap();

    idx.sync(&repo.join("sub")).await.unwrap();

    assert_eq!(
        ids(&idx, "zebra").await,
        ["deep/also.md#1", "keep.md#1"],
        "the repository's .gitignore, .git/info/exclude and .honeignore apply to the subdirectory"
    );
}

#[tokio::test]
async fn test_r12_an_ignore_file_below_the_repository_root_should_apply_to_a_deeper_run_too() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = std::fs::canonicalize(tmp.path()).unwrap().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
    write(&repo, "a/.gitignore", "generated/\n");
    write(&repo, "a/b/keep.md", "# Keep\nokapi notes\n");
    write(&repo, "a/b/generated/g.md", "# G\nokapi notes\n");
    write(&repo, "a/generated/h.md", "# H\nokapi notes\n");
    let idx = DocIndex::in_memory().await.unwrap();

    idx.sync(&repo.join("a/b")).await.unwrap();

    assert_eq!(ids(&idx, "okapi").await, ["keep.md#1"], "a/.gitignore's `generated/` also covers a/b/generated");
}

// ---- R14 -----------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_r14_two_concurrent_syncs_of_one_database_should_both_succeed() {
    for round in 0..10 {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("notes");
        for i in 0..30 {
            write(&dir, &format!("n{i:02}.md"), &format!("# N{i}\nquokka note number {i}\n"));
        }
        let db = tmp.path().join("docs.db");
        let run = |db: std::path::PathBuf, dir: std::path::PathBuf| {
            tokio::spawn(async move {
                let idx = DocIndex::open(&db).await?;
                idx.sync(&dir).await
            })
        };

        let (a, b) = (run(db.clone(), dir.clone()), run(db.clone(), dir.clone()));

        let (a, b) = (a.await.unwrap(), b.await.unwrap());
        a.unwrap_or_else(|e| panic!("round {round}: first sync failed: {e}"));
        b.unwrap_or_else(|e| panic!("round {round}: second sync failed: {e}"));
        let idx = DocIndex::open(&db).await.unwrap();
        assert_eq!(idx.chunk_count().await.unwrap(), 30, "round {round}: every file indexed exactly once");
    }
}

// ---- R16 -----------------------------------------------------------------------------------------------

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

async fn rows(db: &Path, sql: &str, root: &str) -> i64 {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", db.display())).await.unwrap();
    let n = sqlx::query(sql).bind(root).fetch_one(&pool).await.unwrap().get::<i64, _>(0);
    pool.close().await;
    n
}

#[tokio::test]
async fn test_r16_roots_not_synced_for_thirty_days_should_be_pruned_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (old, recent, fresh) = (root.join("old"), root.join("recent"), root.join("fresh"));
    for d in [&old, &recent, &fresh] {
        write(d, "n.md", "# N\nquokka note\n");
    }
    let db = root.join("docs.db");
    {
        let idx = DocIndex::open(&db).await.unwrap();
        for d in [&old, &recent, &fresh] {
            idx.sync(d).await.unwrap();
        }
    }
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", db.display())).await.unwrap();
    for (dir, days) in [(&old, 31), (&recent, 29)] {
        sqlx::query("UPDATE doc_roots SET synced_at = ?1 WHERE root = ?2")
            .bind(now() - days * 86_400)
            .bind(dir.to_string_lossy().into_owned())
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("doc_roots must exist after a sync: {e}"));
    }
    pool.close().await;
    let key = |d: &Path| d.to_string_lossy().into_owned();
    assert_eq!(rows(&db, "SELECT count(*) FROM doc_files WHERE root = ?1", &key(&old)).await, 1, "harness: indexed before");

    let idx = DocIndex::open(&db).await.unwrap();

    assert_eq!(rows(&db, "SELECT count(*) FROM doc_files WHERE root = ?1", &key(&old)).await, 0, "the 31-day-old root is gone");
    assert_eq!(
        rows(&db, "SELECT count(*) FROM doc_chunks c JOIN doc_files f ON f.id = c.file_id WHERE f.root = ?1", &key(&old)).await,
        0,
        "with its chunks"
    );
    assert_eq!(rows(&db, "SELECT count(*) FROM doc_roots WHERE root = ?1", &key(&old)).await, 0);
    for kept in [&recent, &fresh] {
        assert_eq!(rows(&db, "SELECT count(*) FROM doc_files WHERE root = ?1", &key(kept)).await, 1, "{kept:?} stays");
    }
    // The pruned root's text can no longer be retrieved; the others' can.
    idx.sync(&fresh).await.unwrap();
    assert_eq!(ids(&idx, "quokka").await, ["n.md#1"]);
}

// ---- R21 (seam review): the tokenizer rebuild ------------------------------------------------------------

#[tokio::test]
async fn test_r21_a_docs_db_with_another_tokenizer_should_be_rebuilt_on_open() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let notes = root.join("notes");
    write(&notes, "vn.md", "# Hà Nội\nTiếng Việt cơ bản cho người mới\n");
    let db = root.join("docs.db");
    {
        let idx = DocIndex::open(&db).await.unwrap();
        idx.sync(&notes).await.unwrap();
        assert_eq!(ids(&idx, "viet").await, ["vn.md#1"], "harness: the current tokenizer folds Việt to viet");
    }
    // An older docs.db: the FTS table uses the default `unicode61` (remove_diacritics 1).
    let p = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", db.display())).await.unwrap();
    sqlx::query("DROP TABLE doc_chunks_fts").execute(&p).await.unwrap();
    sqlx::query("CREATE VIRTUAL TABLE doc_chunks_fts USING fts5(text, heading, content='doc_chunks', content_rowid='id', tokenize = 'unicode61')")
        .execute(&p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO doc_chunks_fts(doc_chunks_fts) VALUES ('rebuild')").execute(&p).await.unwrap();
    p.close().await;
    {
        let stale = DocIndex::open(&db).await.unwrap_or_else(|e| panic!("{e}"));
        drop(stale);
    }

    // After the open, the table is declared with remove_diacritics 2 again and `viet` matches `Việt`.
    let p = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rw", db.display())).await.unwrap();
    let sql: String = sqlx::query("SELECT sql FROM sqlite_master WHERE name = 'doc_chunks_fts'").fetch_one(&p).await.unwrap().get(0);
    p.close().await;
    assert!(sql.contains("remove_diacritics 2"), "rebuilt with the shared tokenizer's setting: {sql}");
    let idx = DocIndex::open(&db).await.unwrap();
    idx.sync(&notes).await.unwrap();
    assert_eq!(ids(&idx, "viet").await, ["vn.md#1"], "`viet` matches `Việt` after the rebuild");
}
