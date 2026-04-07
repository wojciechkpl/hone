/// Integration tests for the file-system watcher / incremental indexer.
///
/// These tests exercise the full notify → debounce → SqliteIndex pipeline.
/// They rely on real OS file-system events, so they use generous `sleep`
/// margins.  They are inherently slower than unit tests; run them with
/// `cargo test -p hone-index --test watcher`.
///
/// Platform quirks
/// ---------------
/// * macOS FSEvents coalesces rapid writes; we write files and sleep before
///   asserting to give the OS time to flush the event queue.
/// * Linux inotify is reliable but also needs the debounce window to elapse.
/// * Windows is untested in CI; the `RecommendedWatcher` should still work.
///
/// All tests use `flavor = "multi_thread"` so that the background watcher
/// task (spawned inside `IndexWatcher::spawn`) gets scheduled while the test
/// task is blocked on `tokio::time::sleep`.
use std::sync::Arc;
use std::time::Duration;

use hone_index::{IndexWatcher, SqliteIndex};
use tempfile::TempDir;
use tokio::time::sleep;

/// Wait long enough for:
///   1. The OS to emit the fs event (macOS FSEvents can batch with ~1 s latency).
///   2. The debounce window to elapse (200 ms).
///   3. The async watcher task to process the event and call SqliteIndex.
///
/// 2 s is conservative but keeps the suite practical on CI macOS runners.
const SETTLE: Duration = Duration::from_millis(2000);

// ---------------------------------------------------------------------------
// test_watcher_indexes_new_file
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_watcher_indexes_new_file() {
    let dir = TempDir::new().unwrap();
    let index = Arc::new(SqliteIndex::in_memory().await.unwrap());

    let watcher = IndexWatcher::new(Arc::clone(&index), dir.path().to_path_buf());
    let _handle = watcher.spawn().await.expect("watcher should start");

    // Write a new Rust source file.
    let file = dir.path().join("lib.rs");
    std::fs::write(&file, "pub fn hello_world() {}\n").unwrap();

    sleep(SETTLE).await;

    let results = index.search("hello_world", 10).await.unwrap();
    assert!(
        !results.is_empty(),
        "watcher should have indexed hello_world from the new file"
    );
}

// ---------------------------------------------------------------------------
// test_watcher_reindexes_modified_file
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_watcher_reindexes_modified_file() {
    let dir = TempDir::new().unwrap();
    let index = Arc::new(SqliteIndex::in_memory().await.unwrap());

    // Pre-populate the index with the original version of the file.
    let file = dir.path().join("service.rs");
    std::fs::write(&file, "pub fn old_function() {}\n").unwrap();
    index
        .index_file(&file, "pub fn old_function() {}\n")
        .await
        .unwrap();

    let watcher = IndexWatcher::new(Arc::clone(&index), dir.path().to_path_buf());
    let _handle = watcher.spawn().await.expect("watcher should start");

    // Overwrite the file with new content.
    std::fs::write(&file, "pub fn new_function() {}\n").unwrap();

    sleep(SETTLE).await;

    let old_results = index.search("old_function", 10).await.unwrap();
    let new_results = index.search("new_function", 10).await.unwrap();

    assert!(
        old_results.is_empty(),
        "old symbol should have been removed after re-index; got: {old_results:?}"
    );
    assert!(
        !new_results.is_empty(),
        "new symbol should be present after re-index"
    );
}

// ---------------------------------------------------------------------------
// test_watcher_invalidates_deleted_file
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_watcher_invalidates_deleted_file() {
    let dir = TempDir::new().unwrap();
    let index = Arc::new(SqliteIndex::in_memory().await.unwrap());

    // Write and pre-index the file.
    let file = dir.path().join("module.rs");
    std::fs::write(&file, "pub fn doomed() {}\n").unwrap();
    index
        .index_file(&file, "pub fn doomed() {}\n")
        .await
        .unwrap();

    let watcher = IndexWatcher::new(Arc::clone(&index), dir.path().to_path_buf());
    let _handle = watcher.spawn().await.expect("watcher should start");

    // Delete the file.
    std::fs::remove_file(&file).unwrap();

    sleep(SETTLE).await;

    let results = index.search("doomed", 10).await.unwrap();
    assert!(
        results.is_empty(),
        "symbols from a deleted file should be removed; got: {results:?}"
    );
}

// ---------------------------------------------------------------------------
// test_watcher_skips_hidden_dirs
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_watcher_skips_hidden_dirs() {
    let dir = TempDir::new().unwrap();
    let index = Arc::new(SqliteIndex::in_memory().await.unwrap());

    let watcher = IndexWatcher::new(Arc::clone(&index), dir.path().to_path_buf());
    let _handle = watcher.spawn().await.expect("watcher should start");

    // Create a file inside a hidden directory (.git).
    let git_dir = dir.path().join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::write(git_dir.join("hook.rs"), "fn secret_hook() {}\n").unwrap();

    sleep(SETTLE).await;

    let results = index.search("secret_hook", 10).await.unwrap();
    assert!(
        results.is_empty(),
        "files inside hidden directories must never be indexed; got: {results:?}"
    );
}
