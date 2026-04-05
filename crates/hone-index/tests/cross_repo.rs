use std::fs;

use hone_index::MultiRepoIndex;
use tempfile::TempDir;

fn make_repo(src: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("lib.rs"), src).unwrap();
    dir
}

#[test]
fn test_multi_repo_index_searches_all() {
    let repo_a = make_repo("pub fn alpha_function() {}\n");
    let repo_b = make_repo("pub fn beta_function() {}\n");

    let mut idx = MultiRepoIndex::new();
    idx.add_repo("repo_a", repo_a.path()).unwrap();
    idx.add_repo("repo_b", repo_b.path()).unwrap();

    // "function" is a substring of both symbol names
    let results = idx.search("function", None);
    assert_eq!(results.len(), 2);

    let repos: Vec<&str> = results.iter().map(|r| r.repo.as_str()).collect();
    assert!(repos.contains(&"repo_a"));
    assert!(repos.contains(&"repo_b"));
}

#[test]
fn test_repo_scoped_search() {
    let repo_a = make_repo("pub fn alpha_only() {}\n");
    let repo_b = make_repo("pub fn beta_only() {}\n");

    let mut idx = MultiRepoIndex::new();
    idx.add_repo("repo_a", repo_a.path()).unwrap();
    idx.add_repo("repo_b", repo_b.path()).unwrap();

    let results = idx.search("only", Some("repo_a"));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].repo, "repo_a");
    assert_eq!(results[0].result.symbol.name, "alpha_only");
}

#[test]
fn test_cross_repo_symbol_includes_repo_name() {
    let repo_a = make_repo("pub fn unique_alpha() {}\n");

    let mut idx = MultiRepoIndex::new();
    idx.add_repo("my_repo", repo_a.path()).unwrap();

    let results = idx.search("unique_alpha", None);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].repo, "my_repo");
}

#[test]
fn test_add_and_remove_repo() {
    let repo_a = make_repo("pub fn some_fn() {}\n");

    let mut idx = MultiRepoIndex::new();
    idx.add_repo("repo_a", repo_a.path()).unwrap();
    assert_eq!(idx.repo_count(), 1);
    assert!(idx.total_symbol_count() > 0);

    idx.remove_repo("repo_a");
    assert_eq!(idx.repo_count(), 0);
    assert_eq!(idx.total_symbol_count(), 0);
}

#[test]
fn test_multi_repo_empty_search() {
    let idx = MultiRepoIndex::new();
    let results = idx.search("anything", None);
    assert!(results.is_empty());
}
