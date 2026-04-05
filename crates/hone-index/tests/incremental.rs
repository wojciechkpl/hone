use std::path::PathBuf;

use hone_index::SemanticIndex;

fn rs(name: &str) -> PathBuf {
    PathBuf::from(format!("{name}.rs"))
}

#[test]
fn test_incremental_update_only_reindexes_changed() {
    let mut idx = SemanticIndex::new();

    let path_a = rs("module_a");
    let path_b = rs("module_b");

    // Initial indexing.
    idx.index_file(&path_a, "fn a_original() {}\n");
    idx.index_file(&path_b, "fn b_function() {}\n");

    assert_eq!(idx.file_count(), 2);
    assert_eq!(idx.symbol_count(), 2);

    // Invalidate A and re-index with different content.
    idx.invalidate(&path_a);
    assert_eq!(idx.file_count(), 1, "A should have been removed");
    assert_eq!(idx.symbol_count(), 1, "only B symbol should remain");

    // B's symbol is still present.
    assert!(
        !idx.search("b_function").is_empty(),
        "B's symbol should not have been removed"
    );
    assert!(
        idx.search("a_original").is_empty(),
        "A's old symbol should be gone"
    );

    // Re-index A with updated content.
    idx.index_file(&path_a, "fn a_updated() {}\n");
    assert_eq!(idx.file_count(), 2);
    assert_eq!(idx.symbol_count(), 2);

    // Old A symbol is gone, new A symbol is present, B is unchanged.
    assert!(
        !idx.search("a_updated").is_empty(),
        "new A symbol should be present"
    );
    assert!(
        idx.search("a_original").is_empty(),
        "old A symbol should still be absent"
    );
    assert!(
        !idx.search("b_function").is_empty(),
        "B's symbol should remain unchanged"
    );
}
