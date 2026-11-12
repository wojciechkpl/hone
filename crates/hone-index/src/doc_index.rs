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
}

impl Default for DocLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_files: 5000,
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

const SCHEMA: [&str; 5] = [
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
        text, heading, content='doc_chunks', content_rowid='id')",
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
}

impl DocIndex {
    async fn from_pool(pool: sqlx::SqlitePool) -> Result<Self, IndexError> {
        for sql in SCHEMA {
            sqlx::query(sql).execute(&pool).await.map_err(db_err)?;
        }
        sqlx::query(DELETE_TRIGGER)
            .execute(&pool)
            .await
            .map_err(db_err)?;
        Ok(Self {
            pool,
            limits: DocLimits::default(),
            markdown: Arc::new(MarkdownChunker::new()),
            fixed: Arc::new(FixedChunker::default()),
            root: Arc::new(Mutex::new(String::new())),
        })
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
            .create_if_missing(true);
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

    fn walk(root: &Path, limits: DocLimits) -> Vec<Candidate> {
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(true)
            .parents(false)
            .git_global(false)
            .git_exclude(false)
            .require_git(false)
            .follow_links(false)
            .add_custom_ignore_filename(".honeignore");
        let mut found: Vec<Candidate> = Vec::new();
        for entry in builder.build().flatten() {
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

    /// Walk `working_dir` and bring the index up to date.
    pub async fn sync(&self, working_dir: &Path) -> Result<SyncStats, IndexError> {
        let root_path =
            std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_owned());
        let root = root_path.to_string_lossy().into_owned();
        if let Ok(mut r) = self.root.lock() {
            r.clone_from(&root);
        }
        let limits = self.limits;
        let walk_root = root_path.clone();
        let candidates = tokio::task::spawn_blocking(move || Self::walk(&walk_root, limits))
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
        for c in &candidates {
            let previous = stored.remove(&c.rel);
            if let Some((_, _, size, mtime)) = &previous {
                if *size == c.size as i64 && *mtime == c.mtime {
                    stats.unchanged += 1;
                    continue;
                }
            }
            let Ok(bytes) = tokio::fs::read(&c.abs).await else {
                if let Some((id, ..)) = previous {
                    self.remove_file(id).await?;
                    stats.removed += 1;
                }
                continue;
            };
            let Ok(text) = String::from_utf8(bytes) else {
                if let Some((id, ..)) = previous {
                    self.remove_file(id).await?;
                    stats.removed += 1;
                }
                continue;
            };
            let sha = sha256_hex(text.as_bytes());
            if let Some((id, old_sha, ..)) = &previous {
                if *old_sha == sha {
                    sqlx::query("UPDATE doc_files SET size = ?1, mtime = ?2 WHERE id = ?3")
                        .bind(c.size as i64)
                        .bind(c.mtime)
                        .bind(id)
                        .execute(&self.pool)
                        .await
                        .map_err(db_err)?;
                    stats.unchanged += 1;
                    continue;
                }
            }
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
            let mut tx = self.pool.begin().await.map_err(db_err)?;
            let file_id = match &previous {
                Some((id, ..)) => {
                    sqlx::query("DELETE FROM doc_chunks WHERE file_id = ?1")
                        .bind(id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                    sqlx::query("UPDATE doc_files SET sha256 = ?1, size = ?2, mtime = ?3 WHERE id = ?4")
                        .bind(&sha)
                        .bind(c.size as i64)
                        .bind(c.mtime)
                        .bind(id)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                    *id
                }
                None => sqlx::query(
                    "INSERT INTO doc_files (root, path, sha256, size, mtime) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .bind(&root)
                .bind(&c.rel)
                .bind(&sha)
                .bind(c.size as i64)
                .bind(c.mtime)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?
                .last_insert_rowid(),
            };
            for chunk in &chunks {
                sqlx::query(
                    "INSERT INTO doc_chunks (file_id, chunk_id, start_line, end_line, heading, text, sha256)
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
            tx.commit().await.map_err(db_err)?;
            stats.indexed += 1;
        }

        // Files that are gone, ignored, too large, or past the cap.
        for (id, ..) in stored.into_values() {
            self.remove_file(id).await?;
            stats.removed += 1;
        }
        Ok(stats)
    }

    async fn remove_file(&self, id: i64) -> Result<(), IndexError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        sqlx::query("DELETE FROM doc_chunks WHERE file_id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM doc_files WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }
}

/// `fts5-docs`: FTS5 `bm25()` over chunk text and heading (heading weighted 2x).
pub struct Fts5DocsRetriever {
    pool: sqlx::SqlitePool,
    root: Arc<Mutex<String>>,
    id: StrategyId,
}

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
    /// Like [`Retriever::retrieve`], with the location of each hit.
    pub async fn retrieve_located(&self, query: &str, k: usize) -> Result<Vec<DocHit>, IndexError> {
        use hone_memory::fts_query::{match_expression, query_tokens};
        let Some(expr) = match_expression(&query_tokens(query)) else {
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
        Ok(self
            .retrieve_located(query, k)
            .await?
            .into_iter()
            .map(|h| h.hit)
            .collect())
    }
}
