use std::path::Path;

use hone_index::SqliteIndex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rs_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{name}.rs"))
}

const TWO_FN_CONTENT: &str = "fn alpha() {}\nfn beta() {}\n";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sqlite_index_file_and_search() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    idx.index_file(&rs_path("lib"), TWO_FN_CONTENT)
        .await
        .unwrap();

    let results = idx.search("alpha", 10).await.unwrap();
    assert_eq!(results.len(), 1, "expected exactly one result for 'alpha'");
    assert_eq!(results[0].symbol.name, "alpha");
}

#[tokio::test]
async fn test_sqlite_index_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("index.db");

    // First session: index a file.
    {
        let idx = SqliteIndex::open(&db_path).await.unwrap();
        idx.index_file(&rs_path("main"), "fn persist_me() {}\n")
            .await
            .unwrap();
    }

    // Second session: reopen and search.
    {
        let idx = SqliteIndex::open(&db_path).await.unwrap();
        let results = idx.search("persist_me", 10).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "symbol should survive a database close/reopen"
        );
        assert_eq!(results[0].symbol.name, "persist_me");
    }
}

#[tokio::test]
async fn test_sqlite_invalidate() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("lib");

    idx.index_file(&path, TWO_FN_CONTENT).await.unwrap();
    assert_eq!(idx.symbol_count().await.unwrap(), 2);

    idx.invalidate(&path).await.unwrap();

    let results = idx.search("alpha", 10).await.unwrap();
    assert!(
        results.is_empty(),
        "no results expected after invalidation"
    );
    assert_eq!(idx.symbol_count().await.unwrap(), 0);
    assert_eq!(idx.file_count().await.unwrap(), 0);
}

#[tokio::test]
async fn test_sqlite_symbol_at() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("module");

    // alpha is at line 1, beta at line 2.
    idx.index_file(&path, TWO_FN_CONTENT).await.unwrap();

    let sym = idx.symbol_at(&path, 2, 0).await.unwrap();
    assert!(sym.is_some(), "should find a symbol at line 2");
    assert_eq!(sym.unwrap().name, "beta");

    let sym_line1 = idx.symbol_at(&path, 1, 0).await.unwrap();
    assert!(sym_line1.is_some(), "should find a symbol at line 1");
    assert_eq!(sym_line1.unwrap().name, "alpha");
}

#[tokio::test]
async fn test_sqlite_file_count() {
    let idx = SqliteIndex::in_memory().await.unwrap();

    idx.index_file(&rs_path("a"), "fn foo() {}\n")
        .await
        .unwrap();
    idx.index_file(&rs_path("b"), "fn bar() {}\n")
        .await
        .unwrap();

    assert_eq!(idx.file_count().await.unwrap(), 2);
}

#[tokio::test]
async fn test_sqlite_reindex_replaces_symbols() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("lib");

    idx.index_file(&path, "fn old_name() {}\n").await.unwrap();
    assert_eq!(idx.symbol_count().await.unwrap(), 1);

    // Re-index with different content — old symbols must not survive.
    idx.index_file(&path, "fn new_name() {}\n").await.unwrap();

    let old = idx.search("old_name", 10).await.unwrap();
    assert!(old.is_empty(), "stale symbol should be gone after re-index");

    let new = idx.search("new_name", 10).await.unwrap();
    assert_eq!(new.len(), 1);

    // File count must still be 1, not 2.
    assert_eq!(idx.file_count().await.unwrap(), 1);
    assert_eq!(idx.symbol_count().await.unwrap(), 1);
}

#[tokio::test]
async fn test_sqlite_search_scoring_order() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("lib");

    // "compute" is an exact match, "compute_value" is a prefix match.
    idx.index_file(&path, "fn compute() {}\nfn compute_value() {}\n")
        .await
        .unwrap();

    let results = idx.search("compute", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].symbol.name, "compute",
        "exact match should rank first"
    );
    assert!(
        results[0].score > results[1].score,
        "exact score must exceed prefix score"
    );
}

#[tokio::test]
async fn test_sqlite_symbol_at_no_result_before_any_symbol() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("lib");

    // Only symbol is at line 5.
    idx.index_file(&path, "// four blank lines\n\n\n\nfn late() {}\n")
        .await
        .unwrap();

    // Querying before line 5 should return None (or the late symbol if the
    // extractor places it at line 5 — either way line 0 must return None).
    let sym = idx.symbol_at(&path, 0, 0).await.unwrap();
    assert!(
        sym.is_none(),
        "no symbol exists at or before line 0; expected None"
    );
}

#[tokio::test]
async fn test_sqlite_search_limit_respected() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let path = rs_path("big");

    // 5 functions all containing "func".
    let content = (1..=5)
        .map(|i| format!("fn func_{i}() {{}}\n"))
        .collect::<String>();

    idx.index_file(&path, &content).await.unwrap();

    let results = idx.search("func", 3).await.unwrap();
    assert!(
        results.len() <= 3,
        "search must respect the limit parameter"
    );
}

#[tokio::test]
async fn test_sqlite_symbol_at_missing_file_returns_none() {
    let idx = SqliteIndex::in_memory().await.unwrap();
    let sym = idx
        .symbol_at(Path::new("nonexistent.rs"), 1, 0)
        .await
        .unwrap();
    assert!(sym.is_none());
}
