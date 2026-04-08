//! Integration tests for symbol search tools against a real SqliteIndex.
//!
//! The Tool trait wrappers in `bins/hone/src/tools/symbol_tools.rs` are thin
//! shims around the SqliteIndex API, so testing the index directly covers the
//! meaningful behavior without the overhead of constructing full ToolInput /
//! ToolContext values.

use std::path::Path;
use std::sync::Arc;

use hone_index::SqliteIndex;

#[tokio::test]
async fn symbol_search_finds_known_function() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = r#"
fn authenticate(user: &str) -> bool {
    true
}

struct TokenStore;
"#;

    index
        .index_file(Path::new("auth.rs"), content)
        .await
        .expect("index");

    let results = index.search("authenticate", 10).await.expect("search");
    assert!(!results.is_empty(), "expected at least one result");
    assert_eq!(results[0].symbol.name, "authenticate");
}

#[tokio::test]
async fn symbol_search_is_case_insensitive() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = "fn MyHandler() {}\n";
    index
        .index_file(Path::new("handler.rs"), content)
        .await
        .expect("index");

    let results = index.search("myhandler", 10).await.expect("search");
    assert!(
        !results.is_empty(),
        "case-insensitive search should find MyHandler"
    );
    assert_eq!(results[0].symbol.name, "MyHandler");
}

#[tokio::test]
async fn symbol_search_exact_beats_prefix_beats_substring() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = "fn auth() {}\nfn authenticate() {}\nfn pre_auth_check() {}\n";
    index
        .index_file(Path::new("auth.rs"), content)
        .await
        .expect("index");

    let results = index.search("auth", 10).await.expect("search");
    assert!(results.len() >= 3, "expected all three symbols");

    // exact match must come first
    assert_eq!(results[0].symbol.name, "auth");
    assert!(
        results[0].score > results[1].score,
        "exact match score ({}) must beat prefix match score ({})",
        results[0].score,
        results[1].score,
    );
}

#[tokio::test]
async fn symbol_search_empty_results_for_unknown_name() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = "fn foo() {}\n";
    index
        .index_file(Path::new("lib.rs"), content)
        .await
        .expect("index");

    let results = index
        .search("totally_unknown_xyz", 10)
        .await
        .expect("search");
    assert!(results.is_empty());
}

#[tokio::test]
async fn symbol_search_respects_limit() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    // Index 10 functions that all match "foo"
    let content: String = (0..10).map(|i| format!("fn foo_{i}() {{}}\n")).collect();
    index
        .index_file(Path::new("many.rs"), &content)
        .await
        .expect("index");

    let results = index.search("foo", 3).await.expect("search");
    assert!(
        results.len() <= 3,
        "limit=3 must return at most 3 results, got {}",
        results.len()
    );
}

#[tokio::test]
async fn symbol_at_finds_enclosing_function() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = "fn foo() {\n    let x = 1;\n}\n\nfn bar() {\n    let y = 2;\n}\n";
    index
        .index_file(Path::new("test.rs"), content)
        .await
        .expect("index");

    // Line 2 is inside foo()
    let sym = index
        .symbol_at(Path::new("test.rs"), 2, 1)
        .await
        .expect("symbol_at");
    assert!(sym.is_some(), "expected a symbol at line 2");
    assert_eq!(sym.unwrap().name, "foo");
}

#[tokio::test]
async fn symbol_at_returns_none_before_any_declaration() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let content = "// preamble comment\nfn foo() {}\n";
    index
        .index_file(Path::new("test.rs"), content)
        .await
        .expect("index");

    // Line 1 is before any function declaration
    let sym = index
        .symbol_at(Path::new("test.rs"), 1, 1)
        .await
        .expect("symbol_at");
    assert!(sym.is_none(), "line 1 has no enclosing symbol");
}

#[tokio::test]
async fn symbol_at_returns_none_for_unindexed_file() {
    let index = SqliteIndex::in_memory().await.expect("open index");

    let sym = index
        .symbol_at(Path::new("nonexistent.rs"), 5, 1)
        .await
        .expect("symbol_at");
    assert!(sym.is_none());
}

#[tokio::test]
async fn symbol_search_across_multiple_files() {
    let index = Arc::new(SqliteIndex::in_memory().await.expect("open index"));

    index
        .index_file(Path::new("a.rs"), "fn greet() {}\n")
        .await
        .expect("index a");
    index
        .index_file(Path::new("b.rs"), "fn greet_user() {}\n")
        .await
        .expect("index b");

    let results = index.search("greet", 10).await.expect("search");
    assert_eq!(results.len(), 2, "should find symbols from both files");

    let names: Vec<&str> = results.iter().map(|r| r.symbol.name.as_str()).collect();
    assert!(names.contains(&"greet"));
    assert!(names.contains(&"greet_user"));
}
