use std::path::PathBuf;

use hone_index::SemanticIndex;

fn rs(name: &str) -> PathBuf {
    PathBuf::from(format!("{name}.rs"))
}

#[test]
fn test_search_exact_match_first() {
    let mut idx = SemanticIndex::new();
    let path = rs("funcs");
    // Three functions: "foo" (exact), "foobar" (prefix), "barfoo" (suffix).
    let content = "fn foo() {}\nfn foobar() {}\nfn barfoo() {}\n";
    idx.index_file(&path, content);

    let results = idx.search("foo");
    assert_eq!(results.len(), 3, "all three should match");

    // The exact match must be first.
    assert_eq!(
        results[0].symbol.name, "foo",
        "exact match 'foo' must be first, got {:?}",
        results.iter().map(|r| &r.symbol.name).collect::<Vec<_>>()
    );

    // Prefix match should outrank the substring match.
    assert_eq!(results[1].symbol.name, "foobar", "prefix match should be second");
    assert_eq!(results[2].symbol.name, "barfoo", "substring match should be last");
}

#[test]
fn test_search_case_insensitive() {
    let mut idx = SemanticIndex::new();
    let path = rs("types");
    idx.index_file(&path, "pub struct MyStruct {}\n");

    // Search with all-lowercase query.
    let results = idx.search("mystruct");
    assert_eq!(results.len(), 1, "case-insensitive search should find MyStruct");
    assert_eq!(results[0].symbol.name, "MyStruct");

    // Mixed-case query.
    let results = idx.search("MYSTRUCT");
    assert_eq!(results.len(), 1, "upper-case query should also find MyStruct");

    // Partial lowercase.
    let results = idx.search("my");
    assert_eq!(results.len(), 1, "prefix search 'my' should find MyStruct");
}
