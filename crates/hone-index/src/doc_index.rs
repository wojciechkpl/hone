//! The document index: a SQLite database (with an FTS5 table over chunk text and
//! heading) of the chunks of a working directory's notes and docs, brought up
//! to date incrementally, and the `fts5-docs` retriever over it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use hone_proto::{Slot, SourceRef, StrategyId, canonical_json_sha256, sha256_hex};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::doc_chunkers::{FixedChunker, MarkdownChunker};
use crate::docs::{Chunker, Document, Hit, Retriever};
use crate::error::IndexError;

fn db_err(e: impl std::fmt::Display) -> IndexError {
    IndexError::Database(e.to_string())
}

/// What the walk takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocLimits {
    pub max_file_bytes: u64,
    pub max_files: usize,
    /// The walk stops after visiting this many entries (files of any kind and
    /// directories).
    pub max_walk_entries: usize,
}

impl Default for DocLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_files: 5000,
            max_walk_entries: 50_000,
        }
    }
}

/// What one `sync` did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    /// Files (re)chunked.
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
}

const SCHEMA: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS doc_roots (
        root      TEXT    PRIMARY KEY,
        synced_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS doc_files (
        id    INTEGER PRIMARY KEY,
        root  TEXT    NOT NULL,
        path  TEXT    NOT NULL,
        sha256 TEXT   NOT NULL,
        size  INTEGER NOT NULL,
        mtime INTEGER NOT NULL,
        UNIQUE (root, path)
    )",
    "CREATE TABLE IF NOT EXISTS doc_chunks (
        id         INTEGER PRIMARY KEY,
        file_id    INTEGER NOT NULL,
        chunk_id   TEXT    NOT NULL,
        start_line INTEGER NOT NULL,
        end_line   INTEGER NOT NULL,
        heading    TEXT,
        text       TEXT    NOT NULL,
        sha256     TEXT    NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS doc_chunks_file_idx ON doc_chunks(file_id)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS doc_chunks_fts USING fts5(
        text, heading, content='doc_chunks', content_rowid='id',
        tokenize = 'unicode61 remove_diacritics 2')",
    "CREATE TRIGGER IF NOT EXISTS doc_chunks_ai AFTER INSERT ON doc_chunks BEGIN
        INSERT INTO doc_chunks_fts(rowid, text, heading) VALUES (new.id, new.text, new.heading);
     END",
];
const DELETE_TRIGGER: &str =
    "CREATE TRIGGER IF NOT EXISTS doc_chunks_ad AFTER DELETE ON doc_chunks BEGIN
        INSERT INTO doc_chunks_fts(doc_chunks_fts, rowid, text, heading)
        VALUES ('delete', old.id, old.text, old.heading);
     END";

const DOC_EXTENSIONS: [&str; 5] = ["md", "markdown", "txt", "rst", "org"];

struct Candidate {
    rel: String,
    abs: PathBuf,
    size: u64,
    mtime: i64,
    markdown: bool,
}

/// The document index of one database file (or memory); it holds the chunks of
/// every working directory it was synced with, keyed by that directory.
pub struct DocIndex {
    pool: sqlx::SqlitePool,
    limits: DocLimits,
    markdown: Arc<dyn Chunker>,
    fixed: Arc<dyn Chunker>,
    /// The working directory of the latest `sync`; retrievers search it.
    root: Arc<Mutex<String>>,
    /// Held while a sync runs: one at a time per index.
    sync_lock: Arc<tokio::sync::Mutex<()>>,
}

/// Sets a flag when dropped: a dropped (timed out) sync cancels its walk.
struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How long a root may go unsynced before `open` removes it.
const ROOT_MAX_AGE_SECS: i64 = 30 * 86_400;
/// Files written per transaction.
const WRITE_BATCH: usize = 200;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl DocIndex {
    async fn from_pool(pool: sqlx::SqlitePool) -> Result<Self, IndexError> {
        // One transaction, so concurrent opens queue instead of racing.
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.map_err(db_err)?;
        // An index made with another tokenizer is rebuilt (the query side folds
        // diacritics of Latin letters only, as `remove_diacritics 2` does).
        let existing: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = 'doc_chunks_fts'")
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        let retokenize = existing
            .as_deref()
            .is_some_and(|sql| !sql.contains("remove_diacritics 2"));
        if retokenize {
            for sql in [
                "DROP TRIGGER IF EXISTS doc_chunks_ai",
                "DROP TRIGGER IF EXISTS doc_chunks_ad",
                "DROP TABLE doc_chunks_fts",
            ] {
                sqlx::query(sql).execute(&mut *tx).await.map_err(db_err)?;
            }
        }
        for sql in SCHEMA {
            sqlx::query(sql).execute(&mut *tx).await.map_err(db_err)?;
        }
        sqlx::query(DELETE_TRIGGER)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        if retokenize {
            sqlx::query("INSERT INTO doc_chunks_fts(doc_chunks_fts) VALUES ('rebuild')")
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        let index = Self {
            pool,
            limits: DocLimits::default(),
            markdown: Arc::new(MarkdownChunker::new()),
            fixed: Arc::new(FixedChunker::default()),
            root: Arc::new(Mutex::new(String::new())),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        index.prune_old_roots().await?;
        Ok(index)
    }

    /// Remove roots not synced for 30 days, with their files and chunks.
    async fn prune_old_roots(&self) -> Result<(), IndexError> {
        let cutoff = unix_now() - ROOT_MAX_AGE_SECS;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db_err)?;
        // Roots indexed before last-used tracking get a time now (and then age out).
        let now = unix_now();
        for sql in [
            "INSERT OR IGNORE INTO doc_roots (root, synced_at)
             SELECT DISTINCT root, ?1 FROM doc_files",
            "UPDATE doc_roots SET synced_at = ?1 WHERE synced_at IS NULL",
        ] {
            sqlx::query(sql)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        for sql in [
            "DELETE FROM doc_chunks WHERE file_id IN (
                SELECT f.id FROM doc_files f JOIN doc_roots r ON r.root = f.root
                WHERE r.synced_at < ?1)",
            "DELETE FROM doc_files WHERE root IN (SELECT root FROM doc_roots WHERE synced_at < ?1)",
            "DELETE FROM doc_roots WHERE synced_at < ?1",
        ] {
            sqlx::query(sql)
                .bind(cutoff)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)
    }

    /// An index that lives in memory only.
    pub async fn in_memory() -> Result<Self, IndexError> {
        let opts = SqliteConnectOptions::new()
            .in_memory(true)
            .shared_cache(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::from_pool(pool).await
    }

    /// Open (or create) the index database at `path`.
    pub async fn open(path: &Path) -> Result<Self, IndexError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(3));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        Self::from_pool(pool).await
    }

    pub fn with_limits(mut self, limits: DocLimits) -> Self {
        self.limits = limits;
        self
    }

    /// `markdown` chunks `*.md` and `*.markdown`, `fixed` everything else.
    pub fn with_chunkers(mut self, markdown: Arc<dyn Chunker>, fixed: Arc<dyn Chunker>) -> Self {
        self.markdown = markdown;
        self.fixed = fixed;
        self
    }

    /// The retriever over the working directory of the latest `sync`.
    pub fn retriever(&self) -> Fts5DocsRetriever {
        Fts5DocsRetriever {
            pool: self.pool.clone(),
            root: Arc::clone(&self.root),
            deadline: DEFAULT_DEADLINE,
            id: StrategyId {
                slot: Slot::Retrieval,
                name: "fts5-docs".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                params_sha256: canonical_json_sha256(&serde_json::json!({})),
            },
        }
    }

    pub async fn chunk_count(&self) -> Result<usize, IndexError> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM doc_chunks")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(n as usize)
    }

    fn walk(
        root: &Path,
        limits: DocLimits,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Vec<Candidate> {
        use std::sync::atomic::Ordering;
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(true)
            // The repository's ignore files above the working directory, its
            // `.git/info/exclude` and the user's global excludes apply too.
            .parents(true)
            .git_global(true)
            .git_exclude(true)
            .git_ignore(true)
            .require_git(false)
            .follow_links(false)
            .sort_by_file_path(|a, b| a.cmp(b))
            .add_custom_ignore_filename(".honeignore");
        let mut found: Vec<Candidate> = Vec::new();
        let mut visited = 0usize;
        for entry in builder.build().flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            visited += 1;
            if visited > limits.max_walk_entries {
                break;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            let Some(ext) = ext.filter(|e| DOC_EXTENSIONS.contains(&e.as_str())) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > limits.max_file_bytes {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos().min(i64::MAX as u128) as i64);
            found.push(Candidate {
                rel: rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/"),
                abs: path.to_path_buf(),
                size: meta.len(),
                mtime,
                markdown: ext == "md" || ext == "markdown",
            });
        }
        found.sort_by(|a, b| a.rel.cmp(&b.rel));
        found.truncate(limits.max_files);
        found
    }

    /// Walk `working_dir` and bring the index up to date. Waits for a sync that is
    /// already running (one at a time per index).
    pub async fn sync(&self, working_dir: &Path) -> Result<SyncStats, IndexError> {
        let _running = self.sync_lock.lock().await;
        self.sync_locked(working_dir).await
    }

    /// Like [`sync`](Self::sync), but `Ok(None)` at once, with no walk, when a
    /// sync of this index is already running.
    pub async fn sync_if_idle(&self, working_dir: &Path) -> Result<Option<SyncStats>, IndexError> {
        let Ok(_running) = self.sync_lock.try_lock() else {
            return Ok(None);
        };
        self.sync_locked(working_dir).await.map(Some)
    }

    async fn sync_locked(&self, working_dir: &Path) -> Result<SyncStats, IndexError> {
        let root_path =
            std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_owned());
        let root = root_path.to_string_lossy().into_owned();
        if let Ok(mut r) = self.root.lock() {
            r.clone_from(&root);
        }
        let limits = self.limits;
        let walk_root = root_path.clone();
        // Dropping this future (a timed-out sync) stops the walk.
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _guard = CancelOnDrop(Arc::clone(&cancel));
        let walk_cancel = Arc::clone(&cancel);
        let candidates =
            tokio::task::spawn_blocking(move || Self::walk(&walk_root, limits, &walk_cancel))
                .await
                .map_err(db_err)?;

        let stored_rows =
            sqlx::query("SELECT id, path, sha256, size, mtime FROM doc_files WHERE root = ?1")
                .bind(&root)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
        let mut stored: std::collections::HashMap<String, (i64, String, i64, i64)> = stored_rows
            .iter()
            .map(|r| {
                (
                    r.get::<String, _>("path"),
                    (
                        r.get::<i64, _>("id"),
                        r.get::<String, _>("sha256"),
                        r.get::<i64, _>("size"),
                        r.get::<i64, _>("mtime"),
                    ),
                )
            })
            .collect();

        let mut stats = SyncStats::default();
        let mut batch: Vec<Write> = Vec::new();
        for c in &candidates {
            let previous = stored.remove(&c.rel);
            if let Some((_, _, size, mtime)) = &previous {
                if *size == c.size as i64 && *mtime == c.mtime {
                    stats.unchanged += 1;
                    continue;
                }
            }
            let text = match tokio::fs::read(&c.abs).await {
                Ok(bytes) => String::from_utf8(bytes).ok(),
                Err(_) => None,
            };
            let Some(text) = text else {
                // Unreadable or not UTF-8: not indexed (and removed if it was).
                if previous.is_some() {
                    batch.push(Write::Remove {
                        path: c.rel.clone(),
                    });
                    stats.removed += 1;
                }
                continue;
            };
            let sha = sha256_hex(text.as_bytes());
            if previous.as_ref().is_some_and(|(_, old, ..)| *old == sha) {
                batch.push(Write::Touch {
                    path: c.rel.clone(),
                    size: c.size as i64,
                    mtime: c.mtime,
                });
                stats.unchanged += 1;
            } else {
                let doc = Document {
                    uri: c.rel.clone(),
                    text,
                    sha256: sha.clone(),
                };
                let chunks = if c.markdown {
                    self.markdown.chunk(&doc)
                } else {
                    self.fixed.chunk(&doc)
                };
                batch.push(Write::Index {
                    path: c.rel.clone(),
                    sha,
                    size: c.size as i64,
                    mtime: c.mtime,
                    chunks,
                });
                stats.indexed += 1;
            }
            if batch.len() >= WRITE_BATCH {
                self.write_batch(&root, std::mem::take(&mut batch)).await?;
            }
        }

        // Files that are gone, ignored, too large, or past the cap.
        for (path, _) in stored {
            batch.push(Write::Remove { path });
            stats.removed += 1;
            if batch.len() >= WRITE_BATCH {
                self.write_batch(&root, std::mem::take(&mut batch)).await?;
            }
        }
        self.write_batch(&root, batch).await?;
        self.touch_root(&root).await?;
        Ok(stats)
    }

    /// One `BEGIN IMMEDIATE` transaction for a batch of file writes (upserts, so
    /// a concurrent sync of the same directory can't conflict).
    async fn write_batch(&self, root: &str, batch: Vec<Write>) -> Result<(), IndexError> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(db_err)?;
        for write in batch {
            match write {
                Write::Touch { path, size, mtime } => {
                    sqlx::query(
                        "UPDATE doc_files SET size = ?1, mtime = ?2 WHERE root = ?3 AND path = ?4",
                    )
                    .bind(size)
                    .bind(mtime)
                    .bind(root)
                    .bind(path)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
                Write::Remove { path } => {
                    sqlx::query(
                        "DELETE FROM doc_chunks WHERE file_id IN
                         (SELECT id FROM doc_files WHERE root = ?1 AND path = ?2)",
                    )
                    .bind(root)
                    .bind(&path)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                    sqlx::query("DELETE FROM doc_files WHERE root = ?1 AND path = ?2")
                        .bind(root)
                        .bind(path)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                }
                Write::Index {
                    path,
                    sha,
                    size,
                    mtime,
                    chunks,
                } => {
                    let file_id: i64 = sqlx::query_scalar(
                        "INSERT INTO doc_files (root, path, sha256, size, mtime)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(root, path) DO UPDATE SET
                             sha256 = excluded.sha256, size = excluded.size, mtime = excluded.mtime
                         RETURNING id",
                    )
                    .bind(root)
                    .bind(&path)
                    .bind(&sha)
                    .bind(size)
                    .bind(mtime)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_err)?;
                    sqlx::query("DELETE FROM doc_chunks WHERE file_id = ?1")
                        .bind(file_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                    for chunk in &chunks {
                        sqlx::query(
                            "INSERT INTO doc_chunks
                             (file_id, chunk_id, start_line, end_line, heading, text, sha256)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        )
                        .bind(file_id)
                        .bind(&chunk.id)
                        .bind(i64::from(chunk.start_line))
                        .bind(i64::from(chunk.end_line))
                        .bind(&chunk.heading)
                        .bind(&chunk.text)
                        .bind(&chunk.sha256)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                    }
                }
            }
        }
        tx.commit().await.map_err(db_err)
    }

    /// Record that `root` was synced just now.
    async fn touch_root(&self, root: &str) -> Result<(), IndexError> {
        sqlx::query(
            "INSERT INTO doc_roots (root, synced_at) VALUES (?1, ?2)
             ON CONFLICT(root) DO UPDATE SET synced_at = excluded.synced_at",
        )
        .bind(root)
        .bind(unix_now())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

/// One file's change of a sync batch.
enum Write {
    Touch {
        path: String,
        size: i64,
        mtime: i64,
    },
    Remove {
        path: String,
    },
    Index {
        path: String,
        sha: String,
        size: i64,
        mtime: i64,
        chunks: Vec<crate::docs::Chunk>,
    },
}

/// `fts5-docs`: FTS5 `bm25()` over chunk text and heading (heading weighted 2x).
pub struct Fts5DocsRetriever {
    pool: sqlx::SqlitePool,
    root: Arc<Mutex<String>>,
    deadline: std::time::Duration,
    id: StrategyId,
}

/// Ranking longer than this returns no hits.
const DEFAULT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(500);

/// A hit with where it came from, for rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct DocHit {
    pub hit: Hit,
    /// Path relative to the working directory.
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub heading: Option<String>,
}

impl Fts5DocsRetriever {
    /// Ranking longer than `deadline` returns no hits instead of an error (the
    /// turn proceeds without the retrieval block). The default is 500 ms.
    pub fn with_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// The hits for `query`, best first, at most `k` (also available through
    /// the [`Retriever`] trait).
    pub async fn retrieve(&self, query: &str, k: usize) -> Result<Vec<Hit>, IndexError> {
        Ok(self
            .retrieve_located(query, k)
            .await?
            .into_iter()
            .map(|h| h.hit)
            .collect())
    }

    /// Like [`Retriever::retrieve`], with the location of each hit.
    pub async fn retrieve_located(&self, query: &str, k: usize) -> Result<Vec<DocHit>, IndexError> {
        if self.deadline.is_zero() {
            return Ok(Vec::new());
        }
        match tokio::time::timeout(self.deadline, self.rank(query, k)).await {
            Ok(result) => result,
            Err(_) => {
                tracing::debug!("document ranking passed its deadline; no retrieval block");
                Ok(Vec::new())
            }
        }
    }

    async fn rank(&self, query: &str, k: usize) -> Result<Vec<DocHit>, IndexError> {
        // The shared tokenizer; with more than 32 tokens the rarest 32 by the
        // number of chunks holding them in this index.
        use hone_memory::fts_query::{MAX_QUERY_TOKENS, match_expression, query_tokens, rarest};
        let mut tokens = query_tokens(query);
        if tokens.len() > MAX_QUERY_TOKENS {
            let mut frequency = Vec::with_capacity(tokens.len());
            for token in &tokens {
                let n: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM doc_chunks_fts WHERE doc_chunks_fts MATCH ?1",
                )
                .bind(format!("\"{token}\""))
                .fetch_one(&self.pool)
                .await
                .map_err(db_err)?;
                frequency.push(n as u64);
            }
            // A token no chunk holds matches nothing: it never takes a slot.
            let (kept, freq): (Vec<String>, Vec<u64>) = tokens
                .into_iter()
                .zip(frequency)
                .filter(|(_, n)| *n > 0)
                .unzip();
            tokens = rarest(kept, &freq);
        }
        let Some(expr) = match_expression(&tokens) else {
            return Ok(Vec::new());
        };
        let root = self.root.lock().map(|r| r.clone()).unwrap_or_default();
        if k == 0 || root.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT c.chunk_id, c.text, c.sha256, f.path, c.start_line, c.end_line, c.heading,
                    -bm25(doc_chunks_fts, 1.0, 2.0) AS score
             FROM doc_chunks_fts
             JOIN doc_chunks c ON c.id = doc_chunks_fts.rowid
             JOIN doc_files f ON f.id = c.file_id
             WHERE doc_chunks_fts MATCH ?1 AND f.root = ?2
             ORDER BY bm25(doc_chunks_fts, 1.0, 2.0), c.id
             LIMIT ?3",
        )
        .bind(expr)
        .bind(root)
        .bind(k.min(i64::MAX as usize) as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|r| DocHit {
                hit: Hit {
                    source: SourceRef::Chunk {
                        index: "docs".to_owned(),
                        chunk_id: r.get("chunk_id"),
                        sha256: r.get("sha256"),
                    },
                    text: r.get("text"),
                    score: r.get::<f64, _>("score") as f32,
                },
                path: r.get("path"),
                start_line: r.get::<i64, _>("start_line") as u32,
                end_line: r.get::<i64, _>("end_line") as u32,
                heading: r.get("heading"),
            })
            .collect())
    }
}

#[async_trait]
impl Retriever for Fts5DocsRetriever {
    fn id(&self) -> &StrategyId {
        &self.id
    }

    async fn retrieve(&self, query: &str, k: usize) -> Result<Vec<Hit>, IndexError> {
        Fts5DocsRetriever::retrieve(self, query, k).await
    }
}
