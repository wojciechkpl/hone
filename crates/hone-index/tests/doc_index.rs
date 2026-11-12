#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R4 and R5 of `docs/plans/doc-retrieval-spec.md`: the document index (a SQLite
//! database with an FTS5 table, incremental by hash) and the `fts5-docs` retriever.
//!
//! API required (new, in `hone_index`, on top of `tests/doc_chunkers.rs`):
//!
//! ```ignore
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! pub struct DocLimits { pub max_file_bytes: u64, pub max_files: usize }   // Default: 1 MiB, 5000
//!
//! #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
//! pub struct SyncStats { pub indexed: usize, pub unchanged: usize, pub removed: usize }
//!
//! pub struct DocIndex { .. }
//! impl DocIndex {
//!     pub async fn in_memory() -> Result<Self, IndexError>;
//!     pub async fn open(path: &Path) -> Result<Self, IndexError>;     // created if missing
//!     pub fn with_limits(self, limits: DocLimits) -> Self;
//!     /// Replace the chunkers: `markdown` for *.md / *.markdown, `fixed` for the rest.
//!     pub fn with_chunkers(self, markdown: Arc<dyn Chunker>, fixed: Arc<dyn Chunker>) -> Self;
//!     /// Walk `working_dir` and bring the index up to date. `indexed` counts files (re)chunked.
//!     pub async fn sync(&self, working_dir: &Path) -> Result<SyncStats, IndexError>;
//!     pub async fn chunk_count(&self) -> Result<usize, IndexError>;
//!     pub fn retriever(&self) -> Fts5DocsRetriever;
//! }
//! pub struct Fts5DocsRetriever { .. }   // impl Retriever; id(): name "fts5-docs", slot Retrieval
//! ```
//!
//! Walk rules: files with extension md, markdown, txt, rst or org (case-insensitive);
//! `.gitignore` (also when the directory is not a git repository) and `.honeignore`
//! (same syntax) are respected; hidden files and directories are skipped; files larger than
//! `max_file_bytes` are skipped; files are visited in sorted path order and at most
//! `max_files` are indexed. Document uris are paths relative to `working_dir`, with `/`.
//! Incremental: a file with the same size and mtime as stored is unchanged; otherwise its hash
//! decides (same hash: unchanged, the stored mtime is refreshed; different: re-chunked).
//! A file that disappeared (or became ignored or too large) is removed with its chunks.
//!
//! Retrieval (R5): prompt words (letters, digits, `_`; at least 3 characters; English stop words
//! dropped; at most 32) are each quoted and OR-ed, so FTS5 syntax is never interpreted; ranking
//! is `bm25()` over chunk text and heading with the heading weighted 2x; the score is
//! `-bm25()` (higher is better, always > 0 for a returned hit); hits come best first, at most
//! `k`, as `SourceRef::Chunk { index: "docs", chunk_id, sha256 }` with the chunk text.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hone_index::{Chunk, Chunker, DocIndex, DocLimits, Document, MarkdownChunker, Retriever, SyncStats};
use hone_proto::{SourceRef, sha256_hex};

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

struct Counting {
    inner: MarkdownChunker,
    calls: Arc<AtomicUsize>,
}

impl Chunker for Counting {
    fn id(&self) -> &hone_proto::StrategyId {
        self.inner.id()
    }
    fn chunk(&self, doc: &Document) -> Vec<Chunk> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.chunk(doc)
    }
}

async fn counting_index() -> (DocIndex, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let idx = DocIndex::in_memory().await.unwrap().with_chunkers(
        Arc::new(Counting { inner: MarkdownChunker::new(), calls: calls.clone() }),
        Arc::new(hone_index::FixedChunker::default()),
    );
    (idx, calls)
}

async fn ids(idx: &DocIndex, query: &str) -> Vec<String> {
    idx.retriever()
        .retrieve(query, 20)
        .await
        .unwrap()
        .into_iter()
        .map(|h| match h.source {
            SourceRef::Chunk { chunk_id, .. } => chunk_id,
            other => panic!("{other:?}"),
        })
        .collect()
}

// ---- R4: what gets indexed ------------------------------------------------------------------

#[tokio::test]
async fn test_r4_only_document_extensions_should_be_indexed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "# A\nquokka notes\n");
    write(dir.path(), "b.markdown", "# B\nquokka notes\n");
    write(dir.path(), "c.txt", "quokka notes\n");
    write(dir.path(), "d.rst", "quokka notes\n");
    write(dir.path(), "e.org", "* quokka notes\n");
    write(dir.path(), "upper/F.MD", "# F\nquokka notes\n");
    write(dir.path(), "code.rs", "// quokka notes\nfn main() {}\n");
    write(dir.path(), "data.json", "{\"quokka\": \"notes\"}\n");
    let idx = DocIndex::in_memory().await.unwrap();

    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats, SyncStats { indexed: 6, unchanged: 0, removed: 0 });
    let mut got = ids(&idx, "quokka").await;
    got.sort();
    let files: Vec<&str> = got.iter().map(|c| c.split('#').next().unwrap()).collect();
    assert_eq!(files, ["a.md", "b.markdown", "c.txt", "d.rst", "e.org", "upper/F.MD"]);
}

#[tokio::test]
async fn test_r4_gitignore_and_honeignore_and_hidden_paths_should_be_skipped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    write(dir.path(), ".gitignore", "build/\n*.draft.md\n");
    write(dir.path(), ".honeignore", "private/\n");
    write(dir.path(), "keep.md", "# Keep\nzebra stripes\n");
    write(dir.path(), "build/out.md", "# Out\nzebra stripes\n");
    write(dir.path(), "notes.draft.md", "# Draft\nzebra stripes\n");
    write(dir.path(), "private/secret.md", "# Secret\nzebra stripes\n");
    write(dir.path(), ".hidden/h.md", "# Hidden\nzebra stripes\n");
    write(dir.path(), ".dotfile.md", "# Dot\nzebra stripes\n");
    write(dir.path(), "sub/ok.md", "# Ok\nzebra stripes\n");
    let idx = DocIndex::in_memory().await.unwrap();

    idx.sync(dir.path()).await.unwrap();

    let mut got: Vec<String> = ids(&idx, "zebra").await.into_iter().map(|c| c.split('#').next().unwrap().to_owned()).collect();
    got.sort();
    assert_eq!(got, ["keep.md", "sub/ok.md"]);
}

#[tokio::test]
async fn test_r4_gitignore_should_apply_without_a_git_repository_too() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".gitignore", "ignored.md\n");
    write(dir.path(), "ignored.md", "# I\nwombat\n");
    write(dir.path(), "seen.md", "# S\nwombat\n");
    let idx = DocIndex::in_memory().await.unwrap();

    idx.sync(dir.path()).await.unwrap();

    assert_eq!(ids(&idx, "wombat").await, ["seen.md#1"]);
}

#[tokio::test]
async fn test_r4_limits_should_default_to_one_mib_and_five_thousand_files() {
    let l = DocLimits::default();
    assert_eq!((l.max_file_bytes, l.max_files), (1024 * 1024, 5000));
}

#[tokio::test]
async fn test_r4_files_over_the_size_cap_should_be_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "small.md", "# Small\nplatypus\n");
    write(dir.path(), "big.md", &format!("# Big\nplatypus\n{}\n", "x".repeat(4000)));
    let idx = DocIndex::in_memory().await.unwrap().with_limits(DocLimits { max_file_bytes: 1000, max_files: 100 });

    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats.indexed, 1);
    assert_eq!(ids(&idx, "platypus").await, ["small.md#1"]);
}

#[tokio::test]
async fn test_r4_indexing_should_stop_at_the_file_cap_in_sorted_path_order() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..8 {
        write(dir.path(), &format!("n{i}.md"), &format!("# N{i}\nokapi {i}\n"));
    }
    let idx = DocIndex::in_memory().await.unwrap().with_limits(DocLimits { max_file_bytes: 1 << 20, max_files: 5 });

    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats.indexed, 5);
    let mut got: Vec<String> = ids(&idx, "okapi").await;
    got.sort();
    assert_eq!(got, ["n0.md#1", "n1.md#1", "n2.md#1", "n3.md#1", "n4.md#1"]);
}

// ---- R4: incremental -----------------------------------------------------------------------------------

#[tokio::test]
async fn test_r4_unchanged_files_should_not_be_rechunked() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..4 {
        write(dir.path(), &format!("n{i}.md"), &format!("# N{i}\ntext {i}\n"));
    }
    let (idx, calls) = counting_index().await;

    let first = idx.sync(dir.path()).await.unwrap();
    assert_eq!(first, SyncStats { indexed: 4, unchanged: 0, removed: 0 });
    assert_eq!(calls.load(Ordering::SeqCst), 4);

    let second = idx.sync(dir.path()).await.unwrap();
    assert_eq!(second, SyncStats { indexed: 0, unchanged: 4, removed: 0 });
    assert_eq!(calls.load(Ordering::SeqCst), 4, "nothing was chunked again");
}

#[tokio::test]
async fn test_r4_a_changed_file_should_be_rechunked_and_found_with_its_new_text() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "# A\noriginal narwhal text\n");
    write(dir.path(), "b.md", "# B\nstays the same\n");
    let (idx, calls) = counting_index().await;
    idx.sync(dir.path()).await.unwrap();

    write(dir.path(), "a.md", "# A\nrewritten pangolin text with more words\n");
    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats, SyncStats { indexed: 1, unchanged: 1, removed: 0 });
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(ids(&idx, "narwhal").await.is_empty(), "the old text is gone");
    assert_eq!(ids(&idx, "pangolin").await, ["a.md#1"]);
}

#[tokio::test]
async fn test_r4_a_touched_file_with_the_same_content_should_count_as_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "# A\nsame content\n");
    let (idx, calls) = counting_index().await;
    idx.sync(dir.path()).await.unwrap();

    let file = std::fs::OpenOptions::new().write(true).open(dir.path().join("a.md")).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(120)).unwrap();
    drop(file);
    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats, SyncStats { indexed: 0, unchanged: 1, removed: 0 });
    assert_eq!(calls.load(Ordering::SeqCst), 1, "same hash: not chunked again");
}

#[tokio::test]
async fn test_r4_deleted_ignored_and_newly_oversized_files_should_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "# A\naardvark\n");
    write(dir.path(), "b.md", "# B\naardvark\n");
    write(dir.path(), "c.md", "# C\naardvark\n");
    let idx = DocIndex::in_memory().await.unwrap().with_limits(DocLimits { max_file_bytes: 200, max_files: 100 });
    idx.sync(dir.path()).await.unwrap();
    assert_eq!(idx.chunk_count().await.unwrap(), 3);

    std::fs::remove_file(dir.path().join("a.md")).unwrap();
    write(dir.path(), ".honeignore", "b.md\n");
    write(dir.path(), "c.md", &format!("# C\naardvark\n{}\n", "y".repeat(500)));
    let stats = idx.sync(dir.path()).await.unwrap();

    assert_eq!(stats.removed, 3, "{stats:?}");
    assert_eq!(idx.chunk_count().await.unwrap(), 0);
    assert!(ids(&idx, "aardvark").await.is_empty());
}

#[tokio::test]
async fn test_r4_an_index_file_should_make_the_next_process_incremental_too() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("docs.db");
    let work = dir.path().join("work");
    write(&work, "a.md", "# A\nflamingo\n");
    {
        let idx = DocIndex::open(&db).await.unwrap();
        assert_eq!(idx.sync(&work).await.unwrap().indexed, 1);
    }

    let (calls, idx) = {
        let calls = Arc::new(AtomicUsize::new(0));
        let idx = DocIndex::open(&db).await.unwrap().with_chunkers(
            Arc::new(Counting { inner: MarkdownChunker::new(), calls: calls.clone() }),
            Arc::new(hone_index::FixedChunker::default()),
        );
        (calls, idx)
    };
    let stats = idx.sync(&work).await.unwrap();

    assert_eq!(stats, SyncStats { indexed: 0, unchanged: 1, removed: 0 });
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(ids(&idx, "flamingo").await, ["a.md#1"], "the stored chunks are still searchable");
}

#[tokio::test]
async fn test_r4_text_files_should_use_the_fixed_chunker_and_markdown_the_markdown_one() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "n.txt", "# not a heading in a text file\nlemur\n");
    write(dir.path(), "m.md", "# Heading\nlemur\n## Second\nlemur\n");
    let idx = DocIndex::in_memory().await.unwrap();
    idx.sync(dir.path()).await.unwrap();

    let hits = idx.retriever().retrieve("lemur", 10).await.unwrap();

    let by = |file: &str| hits.iter().filter(|h| matches!(&h.source, SourceRef::Chunk { chunk_id, .. } if chunk_id.starts_with(file))).count();
    assert_eq!(by("n.txt"), 1, "one window for the short text file");
    assert_eq!(by("m.md"), 2, "one chunk per heading section");
}

#[tokio::test]
async fn test_r4_retrieving_before_any_sync_should_return_nothing() {
    let idx = DocIndex::in_memory().await.unwrap();
    assert!(idx.retriever().retrieve("anything", 5).await.unwrap().is_empty());
}

// ---- R5: the retriever -------------------------------------------------------------------------------------

async fn corpus() -> (DocIndex, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "deploy.md", "# Deploy\nTo deploy to staging run the script and wait for the health check.\n");
    write(dir.path(), "backup.md", "# Backups\nNightly backups go to the archive bucket.\n");
    write(dir.path(), "oncall.md", "# On-call\nThe on-call engineer restarts the staging service when deploy fails.\n");
    write(dir.path(), "lunch.md", "# Lunch\nTuesday is pasta day.\n");
    let idx = DocIndex::in_memory().await.unwrap();
    idx.sync(dir.path()).await.unwrap();
    (idx, dir)
}

#[tokio::test]
async fn test_r5_hits_should_be_ranked_best_first_with_positive_scores_and_chunk_sources() {
    let (idx, _d) = corpus().await;

    let hits = idx.retriever().retrieve("deploy staging health check", 10).await.unwrap();

    assert!(hits.len() >= 2);
    match &hits[0].source {
        SourceRef::Chunk { index, chunk_id, sha256 } => {
            assert_eq!(index, "docs");
            assert_eq!(chunk_id, "deploy.md#1");
            assert_eq!(*sha256, sha256_hex(hits[0].text.as_bytes()), "the hash of the chunk text");
        }
        other => panic!("{other:?}"),
    }
    assert!(hits[0].text.contains("health check"));
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score), "best first");
    assert!(hits.iter().all(|h| h.score > 0.0));
}

#[tokio::test]
async fn test_r5_only_chunks_sharing_a_token_with_the_prompt_should_be_returned() {
    let (idx, _d) = corpus().await;
    let got = ids(&idx, "archive bucket").await;
    assert_eq!(got, ["backup.md#1"]);
    assert!(idx.retriever().retrieve("quantum entanglement", 5).await.unwrap().is_empty());
}

#[tokio::test]
async fn test_r5_k_should_cap_the_hits() {
    let (idx, _d) = corpus().await;
    assert_eq!(idx.retriever().retrieve("staging deploy", 1).await.unwrap().len(), 1);
    assert!(idx.retriever().retrieve("staging deploy", 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn test_r5_a_heading_match_should_outrank_the_same_word_in_the_body() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.md", "# Notes\nrollout steps overview\n");
    write(dir.path(), "b.md", "# Rollout\nsteps overview notes\n");
    let idx = DocIndex::in_memory().await.unwrap();
    idx.sync(dir.path()).await.unwrap();

    let hits = idx.retriever().retrieve("rollout", 5).await.unwrap();

    let order: Vec<String> = hits
        .iter()
        .map(|h| match &h.source {
            SourceRef::Chunk { chunk_id, .. } => chunk_id.clone(),
            o => panic!("{o:?}"),
        })
        .collect();
    assert_eq!(order, ["b.md#1", "a.md#1"], "heading weighted twice as much");
    assert!(hits[0].score > hits[1].score);
}

#[tokio::test]
async fn test_r5_fts5_syntax_in_a_prompt_should_never_be_interpreted() {
    let (idx, _d) = corpus().await;
    for prompt in [
        "\"deploy",
        "deploy*",
        "deploy NEAR(staging script) AND",
        "-deploy",
        "title: deploy",
        "deploy OR NOT staging",
        "deploy ^ staging (",
        "deploy' ; DROP TABLE chunks; --",
    ] {
        let got = ids(&idx, prompt).await;
        assert!(got.contains(&"deploy.md#1".to_owned()), "prompt {prompt:?} gave {got:?}");
    }
}

#[tokio::test]
async fn test_r5_prompts_without_usable_tokens_should_return_nothing_without_error() {
    let (idx, _d) = corpus().await;
    for prompt in ["", "   ", "the a of to", "!!! ??? ...", "of an", "ab"] {
        assert!(idx.retriever().retrieve(prompt, 5).await.unwrap().is_empty(), "{prompt:?}");
    }
}

#[tokio::test]
async fn test_r5_the_retriever_should_name_its_strategy() {
    let (idx, _d) = corpus().await;
    let r = idx.retriever();
    assert_eq!(r.id().name, "fts5-docs");
    assert_eq!(r.id().slot, hone_proto::Slot::Retrieval);
}

#[tokio::test]
async fn test_r5_chunks_of_long_sections_should_be_individually_retrievable() {
    let dir = tempfile::tempdir().unwrap();
    let mut text = String::from("# Handbook\n");
    for n in 1..=130 {
        text.push_str(&format!("filler line {n}\n"));
        if n == 100 {
            text.push_str("\nthe needle-word zeppelin lives here\n\n");
        }
    }
    write(dir.path(), "handbook.md", &text);
    let idx = DocIndex::in_memory().await.unwrap();
    idx.sync(dir.path()).await.unwrap();

    let hits = idx.retriever().retrieve("zeppelin", 3).await.unwrap();

    assert_eq!(hits.len(), 1);
    assert!(hits[0].text.contains("zeppelin"));
    assert!(hits[0].text.lines().count() <= 61, "only the piece, not the whole section");
}
