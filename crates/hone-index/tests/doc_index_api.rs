#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R15 and R17 of `docs/plans/doc-retrieval-spec.md` (review round 1). These need new API and
//! so fail to compile until it exists.
//!
//! API required (new, `hone_index`):
//!
//! ```ignore
//! pub struct DocLimits { pub max_file_bytes: u64, pub max_files: usize,
//!                        pub max_walk_entries: usize }   // Default: 1 MiB, 5000, 50_000
//! impl DocIndex {
//!     /// `Ok(None)` at once, doing no walk, when a sync of this index is already running;
//!     /// otherwise like `sync`: `Ok(Some(stats))`. (`sync` itself waits its turn.)
//!     pub async fn sync_if_idle(&self, working_dir: &Path) -> Result<Option<SyncStats>, IndexError>;
//! }
//! impl Fts5DocsRetriever {
//!     /// Ranking longer than `deadline` returns `Ok(vec![])` (the turn proceeds without the
//!     /// retrieval block). The default deadline is 500 ms.
//!     pub fn with_deadline(self, deadline: Duration) -> Self;
//! }
//! ```
//!
//! R15 walk cap: the walk checks a cancellation flag per entry and stops after visiting
//! `max_walk_entries` entries (files of any kind and directories). Write batching (one
//! transaction per 200 files) is an internal cost measure that cannot be observed from here.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hone_index::{Chunk, Chunker, DocIndex, DocLimits, Document, FixedChunker, MarkdownChunker};

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn test_r15_the_walk_cap_should_default_to_fifty_thousand_entries() {
    assert_eq!(DocLimits::default().max_walk_entries, 50_000);
}

#[tokio::test]
async fn test_r15_the_walk_should_stop_after_the_entry_cap_whatever_the_entries_are() {
    let dir = tempfile::tempdir().unwrap();
    // 150 non-document files sort before the 50 documents.
    for i in 0..150 {
        write(dir.path(), &format!("a{i:03}.dat"), "x");
    }
    for i in 0..50 {
        write(dir.path(), &format!("z{i:02}.md"), &format!("# Z{i}\nquokka {i}\n"));
    }
    let capped = DocIndex::in_memory().await.unwrap().with_limits(DocLimits { max_walk_entries: 100, ..Default::default() });
    let open = DocIndex::in_memory().await.unwrap().with_limits(DocLimits { max_walk_entries: 10_000, ..Default::default() });

    let stats = capped.sync(dir.path()).await.unwrap();
    let control = open.sync(dir.path()).await.unwrap();

    assert_eq!(control.indexed, 50, "harness: without the cap all documents are indexed");
    assert_eq!(stats.indexed, 0, "100 entries visited, all of them .dat files");
}

/// A chunker that blocks its first call until released, and counts calls.
struct Blocking {
    inner: MarkdownChunker,
    calls: Arc<AtomicUsize>,
    release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

impl Chunker for Blocking {
    fn id(&self) -> &hone_proto::StrategyId {
        self.inner.id()
    }
    fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = self.release.lock().unwrap().recv_timeout(Duration::from_secs(30));
        }
        self.inner.chunk(doc)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_r15_a_second_sync_while_one_runs_should_not_start_another_walk() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        write(dir.path(), &format!("n{i}.md"), &format!("# N{i}\nquokka {i}\n"));
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let idx = Arc::new(DocIndex::in_memory().await.unwrap().with_chunkers(
        Arc::new(Blocking { inner: MarkdownChunker::new(), calls: calls.clone(), release: Arc::new(Mutex::new(release_rx)) }),
        Arc::new(FixedChunker::default()),
    ));
    let running = {
        let (idx, path) = (idx.clone(), dir.path().to_path_buf());
        tokio::spawn(async move { idx.sync(&path).await })
    };
    let start = std::time::Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(start.elapsed() < Duration::from_secs(10), "the first sync never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let second = tokio::time::timeout(Duration::from_secs(5), idx.sync_if_idle(dir.path()))
        .await
        .expect("sync_if_idle must not wait for the running sync")
        .unwrap();

    assert!(second.is_none(), "a sync is already running: {second:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no second walk chunked anything");
    release_tx.send(()).unwrap();
    let done = running.await.unwrap().unwrap();
    assert_eq!(done.indexed, 3);
    let again = idx.sync_if_idle(dir.path()).await.unwrap().expect("idle again: it runs");
    assert_eq!((again.indexed, again.unchanged), (0, 3));
}

// ---- R17 ---------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_r17_ranking_past_the_deadline_should_return_no_hits_and_no_error() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(dir.path(), &format!("n{i}.md"), &format!("# N{i}\nquokka facts number {i}\n"));
    }
    let idx = DocIndex::in_memory().await.unwrap();
    idx.sync(dir.path()).await.unwrap();

    let generous = idx.retriever().with_deadline(Duration::from_secs(30)).retrieve("quokka facts", 5).await.unwrap();
    let expired = idx.retriever().with_deadline(Duration::ZERO).retrieve("quokka facts", 5).await.unwrap();

    assert_eq!(generous.len(), 5, "harness: with time to spare there are hits");
    assert!(expired.is_empty(), "an exhausted deadline means no retrieval block, not an error: {expired:?}");
}
