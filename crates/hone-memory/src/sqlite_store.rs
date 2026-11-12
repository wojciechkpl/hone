use std::path::Path;

use async_trait::async_trait;
use sqlx::Row;

use crate::bm25::{Bm25Index, Bm25Params};
use crate::embedding::{ScoredMemory, cosine_similarity};
use crate::encrypted::SALT_SENTINEL_ID;
use crate::error::MemoryError;
use crate::store::{MemoryStore, SealedVectorStore};
use crate::types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};

pub struct SqliteStore {
    pool: sqlx::SqlitePool,
    /// Whether the store keeps an FTS5 index (an encrypted store's inner
    /// database holds ciphertext and keeps none).
    fts: bool,
}

/// The highest schema version this binary understands.  Bump this whenever a
/// new migration is added to `run_migrations`.
// crate-external for tests; not stable API
pub const CURRENT_SCHEMA_VERSION: i32 = 3;

/// Maximum number of memories fetched for BM25 scoring.
///
/// BM25 needs the full corpus for IDF; we cap at the most-recent N memories to
/// bound recall cost. Memories older than the Nth most-recent are not
/// searchable.
const MAX_RECALL_CORPUS: usize = 5000;

const CREATE_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS memories (
        id          TEXT    PRIMARY KEY,
        content     TEXT    NOT NULL,
        tags        TEXT    NOT NULL DEFAULT '[]',
        weight      REAL    NOT NULL DEFAULT 1.0,
        created_at  INTEGER NOT NULL,
        decay_at    INTEGER,
        kind        TEXT    NOT NULL DEFAULT 'conversation',
        description TEXT
    )
";

const CREATE_META_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS memory_meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    )
";

// Index serves forget(MemoryFilter::Kind(_)) today and will serve typed recall
// once R7 lands — do not remove as dead weight even if recall ignores it now.
const CREATE_KIND_INDEX_SQL: &str =
    "CREATE INDEX IF NOT EXISTS memories_kind_idx ON memories(kind)";

/// Rank `memories` against `text` with the in-Rust BM25 over their searchable
/// text (description, a newline, content), weighted by the memory weight, and
/// return the best `limit` with a positive score. Shared by `SqliteStore` and
/// `EncryptedStore` so both rank alike.
pub(crate) fn bm25_top(memories: Vec<Memory>, text: &str, limit: usize) -> Vec<Memory> {
    bm25_scored(memories, text, limit)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

/// [`bm25_top`] with each memory's score (BM25 x weight).
pub(crate) fn bm25_scored(memories: Vec<Memory>, text: &str, limit: usize) -> Vec<(f64, Memory)> {
    let texts: Vec<String> = memories
        .iter()
        .map(|m| {
            let desc = m.description.as_deref().unwrap_or_default();
            format!("{desc}\n{}", m.content)
        })
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let index = Bm25Index::build(&refs, Bm25Params::default());

    let mut scored: Vec<(f64, Memory)> = memories
        .into_iter()
        .enumerate()
        .filter_map(|(i, memory)| {
            let bm25 = index.score(i, text);
            if bm25 > 0.0 {
                Some((bm25 * memory.weight, memory))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
}

/// The index text lives in a plain FTS5 table; its rowids are the "docids" of
/// `memories_fts_map`, which maps each memory `id` to its docid. Triggers delete
/// by docid (cheap at any size) and never depend on the rowids of `memories`, so
/// renumbering those (`VACUUM`) cannot misalign the index.
const FTS_TABLE_SQL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
        content, description, tokenize = 'unicode61 remove_diacritics 2')";
const FTS_MAP_SQL: &str = "CREATE TABLE IF NOT EXISTS memories_fts_map (
        id    TEXT    PRIMARY KEY,
        docid INTEGER NOT NULL UNIQUE
    )";

// `record` is INSERT OR REPLACE, whose implicit delete does not fire DELETE
// triggers (recursive_triggers is off), so a BEFORE INSERT trigger removes the
// replaced row's index entry first (a no-op for a fresh id). The update trigger
// fires only when the indexed columns are updated, not on weight changes (decay).
const FTS_TRIGGERS: [(&str, &str); 4] = [
    (
        "memories_fts_bi",
        "CREATE TRIGGER memories_fts_bi BEFORE INSERT ON memories BEGIN
        DELETE FROM memories_fts WHERE rowid = (SELECT docid FROM memories_fts_map WHERE id = new.id);
        DELETE FROM memories_fts_map WHERE id = new.id;
     END",
    ),
    (
        "memories_fts_ai",
        "CREATE TRIGGER memories_fts_ai AFTER INSERT ON memories BEGIN
        INSERT INTO memories_fts(content, description) VALUES (new.content, new.description);
        INSERT INTO memories_fts_map(id, docid) VALUES (new.id, last_insert_rowid());
     END",
    ),
    (
        "memories_fts_ad",
        "CREATE TRIGGER memories_fts_ad AFTER DELETE ON memories BEGIN
        DELETE FROM memories_fts WHERE rowid = (SELECT docid FROM memories_fts_map WHERE id = old.id);
        DELETE FROM memories_fts_map WHERE id = old.id;
     END",
    ),
    (
        "memories_fts_au",
        "CREATE TRIGGER memories_fts_au AFTER UPDATE OF content, description ON memories BEGIN
        DELETE FROM memories_fts WHERE rowid = (SELECT docid FROM memories_fts_map WHERE id = old.id);
        DELETE FROM memories_fts_map WHERE id = old.id;
        INSERT INTO memories_fts(content, description) VALUES (new.content, new.description);
        INSERT INTO memories_fts_map(id, docid) VALUES (new.id, last_insert_rowid());
     END",
    ),
];

fn embedding_to_bytes(emb: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(emb.len() * 4);
    for v in emb {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn to_io_err(e: impl std::fmt::Display) -> MemoryError {
    MemoryError::Io(std::io::Error::other(e.to_string()))
}

fn kind_from_str(s: &str) -> MemoryKind {
    match s {
        "user" => MemoryKind::User,
        "feedback" => MemoryKind::Feedback,
        "project" => MemoryKind::Project,
        "reference" => MemoryKind::Reference,
        "outcome" => MemoryKind::Outcome,
        _ => MemoryKind::Conversation,
    }
}

fn kind_to_str(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Conversation => "conversation",
        MemoryKind::User => "user",
        MemoryKind::Feedback => "feedback",
        MemoryKind::Project => "project",
        MemoryKind::Reference => "reference",
        MemoryKind::Outcome => "outcome",
    }
}

fn row_to_memory(row: &sqlx::sqlite::SqliteRow) -> Result<Memory, MemoryError> {
    row_to_memory_with(row)
}

/// A memory from a row. The `embedding` column is read as it is: for an
/// encrypted store it is NULL (sealed vectors live in `memory_vectors_sealed`).
fn row_to_memory_with(row: &sqlx::sqlite::SqliteRow) -> Result<Memory, MemoryError> {
    let id: String = row.try_get("id").map_err(to_io_err)?;
    let content: String = row.try_get("content").map_err(to_io_err)?;
    let tags_json: String = row.try_get("tags").map_err(to_io_err)?;
    let weight: f64 = row.try_get("weight").map_err(to_io_err)?;
    let created_at: i64 = row.try_get("created_at").map_err(to_io_err)?;
    let decay_at: Option<i64> = row.try_get("decay_at").map_err(to_io_err)?;
    let kind_str: String = row.try_get("kind").map_err(to_io_err)?;
    let kind = kind_from_str(&kind_str);
    // description is nullable; .ok().flatten() converts both SQL NULL and
    // decode errors to None — intentional, the field is non-critical.
    let description: Option<String> = row
        .try_get::<Option<String>, _>("description")
        .ok()
        .flatten();

    let tags: Vec<String> = serde_json::from_str(&tags_json)?;

    let embedding: Option<Vec<f32>> = row
        .try_get::<Option<Vec<u8>>, _>("embedding")
        .ok()
        .flatten()
        .map(|bytes| bytes_to_embedding(&bytes));

    Ok(Memory {
        id,
        content,
        tags,
        weight,
        created_at: created_at as u64,
        decay_at: decay_at.map(|t| t as u64),
        kind,
        description,
        embedding,
    })
}

/// Return the current schema version stored in the `schema_version` table.
///
/// Returns `0` when the table exists but is empty, and propagates a
/// `MemoryError` if the query fails (e.g. the table does not yet exist).
// crate-external for tests; not stable API
pub async fn current_schema_version(pool: &sqlx::SqlitePool) -> Result<i32, MemoryError> {
    let version: Option<i32> =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(pool)
            .await
            .map_err(to_io_err)?;
    Ok(version.unwrap_or(0))
}

/// Check whether `column` already exists in `table`.
///
/// Issues `PRAGMA table_info(table)` and scans the result rows.  Used before
/// each `ALTER TABLE … ADD COLUMN` so we never attempt to add a column that is
/// already present — eliminating both the need to swallow SQLite error messages
/// and any risk of silently absorbing unrelated errors.
async fn column_exists(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
) -> Result<bool, MemoryError> {
    let pragma = format!("PRAGMA table_info({table})");
    let rows = sqlx::query(&pragma)
        .fetch_all(&mut *conn)
        .await
        .map_err(to_io_err)?;
    for row in &rows {
        let name: String = row.try_get("name").map_err(to_io_err)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Run all pending schema migrations and return the final schema version.
///
/// # Migration strategy
///
/// 1. Acquire a single connection from the pool and issue `BEGIN IMMEDIATE`.
///    This write-locks the database immediately: any concurrent opener that also
///    calls `BEGIN IMMEDIATE` will block until we `COMMIT`, after which it will
///    re-read `MAX(version)` and find the schema already up-to-date.  This
///    closes the race window that existed when `CREATE TABLE IF NOT EXISTS` and
///    `MAX(version)` were separate round-trips outside any transaction.
/// 2. Create `schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)`
///    if absent.
/// 3. Read the current `MAX(version)` (0 when the table is empty).
/// 4. For each version from 1 to `CURRENT_SCHEMA_VERSION`, run the corresponding
///    DDL only if the column does not already exist (`PRAGMA table_info`), then
///    stamp the version row with `INSERT OR IGNORE`.
/// 5. `COMMIT`.
///
/// ## Legacy R1 / fresh-db handling
///
/// Both a fresh db (columns already present via `CREATE_TABLE_SQL`) and a
/// legacy R1 db (created by old code that ran `ALTER TABLE` without versioning)
/// present `current == 0` with `kind` + `description` already in the table.
/// `column_exists` detects this and skips the `ALTER TABLE` call, making the
/// migration fully idempotent without touching any SQLite error messages.
///
/// ## applied_at format
///
/// RFC 3339 TEXT (e.g. `"2024-01-02T03:04:05Z"`) is used here rather than
/// Unix-second integers (as stored in `memory_meta.last_decay_at`) because
/// `schema_version` is a small administrative table that humans may inspect
/// directly in an SQLite viewer.
pub async fn run_migrations(pool: &sqlx::SqlitePool) -> Result<i32, MemoryError> {
    // Acquire a dedicated connection for the entire migration.  All DDL runs on
    // this single connection so SQLite's per-connection locking is consistent.
    let mut conn = pool.acquire().await.map_err(to_io_err)?;

    // BEGIN IMMEDIATE write-locks the database right away.  A concurrent opener
    // attempting its own BEGIN IMMEDIATE will block here until we COMMIT, then
    // re-read MAX(version) and find the schema already current.
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *conn)
        .await
        .map_err(to_io_err)?;

    // Run the migration body; if anything fails, roll back before propagating.
    match run_migrations_inner(&mut conn).await {
        Ok(version) => {
            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await
                .map_err(to_io_err)?;
            Ok(version)
        }
        Err(e) => {
            // Best-effort rollback; ignore secondary errors.
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(e)
        }
    }
}

/// Inner migration logic that runs inside the `BEGIN IMMEDIATE` transaction.
async fn run_migrations_inner(conn: &mut sqlx::SqliteConnection) -> Result<i32, MemoryError> {
    // Step 1: ensure the version tracking table exists.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version    INTEGER PRIMARY KEY,
            applied_at TEXT    NOT NULL
        )",
    )
    .execute(&mut *conn)
    .await
    .map_err(to_io_err)?;

    // Step 2: find out where we are.
    let current: Option<i32> =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&mut *conn)
            .await
            .map_err(to_io_err)?;
    let current = current.unwrap_or(0);

    if current >= CURRENT_SCHEMA_VERSION {
        return Ok(current);
    }

    let now_str = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(to_io_err)?;

    // Step 3: apply each missing version in order.
    for v in (current + 1)..=CURRENT_SCHEMA_VERSION {
        match v {
            1 => {
                // v1: add `kind` column.  Skip if already present (fresh db or
                // legacy R1 db that already ran the ALTER outside of versioning).
                if !column_exists(&mut *conn, "memories", "kind").await? {
                    sqlx::query(
                        "ALTER TABLE memories ADD COLUMN kind TEXT NOT NULL DEFAULT 'conversation'",
                    )
                    .execute(&mut *conn)
                    .await
                    .map_err(to_io_err)?;
                }
            }
            2 => {
                // v2: add `description` column.  Same presence-check pattern.
                if !column_exists(&mut *conn, "memories", "description").await? {
                    sqlx::query("ALTER TABLE memories ADD COLUMN description TEXT")
                        .execute(&mut *conn)
                        .await
                        .map_err(to_io_err)?;
                }
            }
            3 => {
                // v3: add `embedding` BLOB column for semantic recall.
                if !column_exists(&mut *conn, "memories", "embedding").await? {
                    sqlx::query("ALTER TABLE memories ADD COLUMN embedding BLOB")
                        .execute(&mut *conn)
                        .await
                        .map_err(to_io_err)?;
                }
            }
            _ => {
                // Future migrations go here.
            }
        }

        // Stamp the version row.  INSERT OR IGNORE ensures that if two openers
        // somehow reach this point concurrently (impossible under IMMEDIATE, but
        // defensive), only the first writer wins.
        sqlx::query("INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)")
            .bind(v)
            .bind(&now_str)
            .execute(&mut *conn)
            .await
            .map_err(to_io_err)?;
    }

    Ok(CURRENT_SCHEMA_VERSION)
}

impl SqliteStore {
    /// Open or create a SQLite database at the given path.
    ///
    /// Decay runs at most once per 24 hours. To force-decay in tests use
    /// [`SqliteStore::open_with_decay_interval`] with `interval_secs = 0`.
    pub async fn open(path: &Path) -> Result<Self, MemoryError> {
        Self::open_with_decay_interval(path, 86_400).await
    }

    /// Like [`SqliteStore::open`] but lets the caller control the minimum
    /// number of seconds between automatic decay passes.  Pass `0` to always
    /// decay (useful in tests).
    pub async fn open_with_decay_interval(
        path: &Path,
        interval_secs: u64,
    ) -> Result<Self, MemoryError> {
        Self::open_inner(path, interval_secs, true).await
    }

    /// Create the side table of sealed vectors (`EncryptedStore`) and the trigger
    /// that removes a memory's sealed vector when the memory is deleted
    /// (forget, decay). Not a schema-version change: both are `IF NOT EXISTS`.
    async fn ensure_sealed_table(&self) -> Result<(), MemoryError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_vectors_sealed
             (id TEXT PRIMARY KEY, blob BLOB NOT NULL)",
        )
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?;
        sqlx::query(
            "CREATE TRIGGER IF NOT EXISTS memory_vectors_sealed_ad AFTER DELETE ON memories
             BEGIN DELETE FROM memory_vectors_sealed WHERE id = old.id; END",
        )
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?;
        Ok(())
    }

    /// Open (creating if needed) the database WITHOUT an FTS5 index: no FTS table,
    /// mapping table or triggers are created. It is the inner store of an
    /// `EncryptedStore`, whose content is ciphertext and cannot be indexed;
    /// `recall_fts5` on it ranks with the in-Rust BM25.
    pub async fn open_without_fts(path: &Path) -> Result<Self, MemoryError> {
        Self::open_inner(path, 86_400, false).await
    }

    async fn open_inner(path: &Path, interval_secs: u64, fts: bool) -> Result<Self, MemoryError> {
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let pool = sqlx::SqlitePool::connect(&url).await.map_err(to_io_err)?;
        sqlx::query(CREATE_TABLE_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        run_migrations(&pool).await?;
        sqlx::query(CREATE_KIND_INDEX_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        sqlx::query(CREATE_META_TABLE_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;

        let mut store = Self { pool, fts };
        if fts {
            // An encrypted database (it holds the encryption sentinel) never
            // gets an FTS index, however it is opened.
            let encrypted: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE id = ?1")
                .bind(crate::encrypted::SALT_SENTINEL_ID)
                .fetch_one(&store.pool)
                .await
                .map_err(to_io_err)?;
            if encrypted > 0 {
                store.fts = false;
            } else {
                store.ensure_fts().await?;
            }
        } else {
            store.drop_fts().await?;
        }

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let last_decay: u64 = sqlx::query_scalar::<_, String>(
            "SELECT value FROM memory_meta WHERE key = 'last_decay_at'",
        )
        .fetch_optional(&store.pool)
        .await
        .map_err(to_io_err)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

        if now_secs.saturating_sub(last_decay) >= interval_secs {
            store.decay(0.95, now_secs).await?;
            sqlx::query(
                "INSERT INTO memory_meta (key, value) VALUES ('last_decay_at', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(now_secs.to_string())
            .execute(&store.pool)
            .await
            .map_err(to_io_err)?;
        }

        Ok(store)
    }

    /// Open an in-memory SQLite database (for testing).
    pub async fn in_memory() -> Result<Self, MemoryError> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .map_err(to_io_err)?;
        sqlx::query(CREATE_TABLE_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        // For a brand-new in-memory DB, CREATE_TABLE_SQL already includes the
        // new columns — run_migrations stamps the schema_version rows.
        run_migrations(&pool).await?;
        sqlx::query(CREATE_KIND_INDEX_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        let store = Self { pool, fts: true };
        store.ensure_fts().await?;
        Ok(store)
    }

    /// Make the FTS5 index current, in one `BEGIN IMMEDIATE` transaction (so
    /// concurrent opens queue instead of racing): the tables, the triggers (only
    /// when missing or when their stored SQL differs), the `secure-delete` option,
    /// and a rebuild when the index disagrees with `memories`.
    async fn ensure_fts(&self) -> Result<(), MemoryError> {
        self.setup_fts(false).await.map(|_| ())
    }

    /// The setup behind [`ensure_fts`](Self::ensure_fts) and
    /// [`reindex_fts`](Self::reindex_fts); `force` rebuilds regardless. Returns the
    /// number of memories indexed.
    async fn setup_fts(&self, force: bool) -> Result<usize, MemoryError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(to_io_err)?;

        // An index from before the mapping table (keyed on the memory id inside
        // the FTS table) is replaced.
        let existing: Option<String> =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = 'memories_fts'")
                .fetch_optional(&mut *tx)
                .await
                .map_err(to_io_err)?;
        if existing
            .as_deref()
            .is_some_and(|sql| sql.contains("UNINDEXED"))
        {
            for (name, _) in FTS_TRIGGERS {
                sqlx::query(&format!("DROP TRIGGER IF EXISTS {name}"))
                    .execute(&mut *tx)
                    .await
                    .map_err(to_io_err)?;
            }
            sqlx::query("DROP TABLE memories_fts")
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
        }
        sqlx::query(FTS_TABLE_SQL)
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        sqlx::query(FTS_MAP_SQL)
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        for (name, sql) in FTS_TRIGGERS {
            let current: Option<String> = sqlx::query_scalar(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            )
            .bind(name)
            .fetch_optional(&mut *tx)
            .await
            .map_err(to_io_err)?;
            if current.as_deref() != Some(sql) {
                sqlx::query(&format!("DROP TRIGGER IF EXISTS {name}"))
                    .execute(&mut *tx)
                    .await
                    .map_err(to_io_err)?;
                sqlx::query(sql)
                    .execute(&mut *tx)
                    .await
                    .map_err(to_io_err)?;
            }
        }
        // Deleted text leaves the index's stored data.
        let secure: Option<i64> =
            sqlx::query_scalar("SELECT v FROM memories_fts_config WHERE k = 'secure-delete'")
                .fetch_optional(&mut *tx)
                .await
                .map_err(to_io_err)?;
        if secure != Some(1) {
            sqlx::query("INSERT INTO memories_fts(memories_fts, rank) VALUES ('secure-delete', 1)")
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
        }

        let memories: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
            .fetch_one(&mut *tx)
            .await
            .map_err(to_io_err)?;
        let consistent = !force && {
            let mapped: i64 = sqlx::query_scalar("SELECT count(*) FROM memories_fts_map")
                .fetch_one(&mut *tx)
                .await
                .map_err(to_io_err)?;
            let indexed: i64 = sqlx::query_scalar("SELECT count(*) FROM memories_fts_docsize")
                .fetch_one(&mut *tx)
                .await
                .map_err(to_io_err)?;
            let unmapped: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM memories WHERE id NOT IN (SELECT id FROM memories_fts_map)",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(to_io_err)?;
            mapped == memories && indexed == memories && unmapped == 0
        };
        if !consistent {
            sqlx::query("DELETE FROM memories_fts")
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
            sqlx::query("DELETE FROM memories_fts_map")
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
            sqlx::query(
                "INSERT INTO memories_fts(rowid, content, description)
                 SELECT rowid, content, description FROM memories",
            )
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
            sqlx::query("INSERT INTO memories_fts_map(id, docid) SELECT id, rowid FROM memories")
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
        }
        tx.commit().await.map_err(to_io_err)?;
        Ok(memories as usize)
    }

    /// Remove any FTS5 table, mapping table and triggers (one transaction).
    async fn drop_fts(&self) -> Result<(), MemoryError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(to_io_err)?;
        for (name, _) in FTS_TRIGGERS {
            sqlx::query(&format!("DROP TRIGGER IF EXISTS {name}"))
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
        }
        sqlx::query("DROP TABLE IF EXISTS memories_fts")
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        sqlx::query("DROP TABLE IF EXISTS memories_fts_map")
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        tx.commit().await.map_err(to_io_err)
    }

    /// Rebuild the FTS5 index from `memories`, atomically (creating the tables
    /// and triggers when missing). Returns the number of memories indexed; `0`
    /// for a store opened without an index.
    pub async fn reindex_fts(&self) -> Result<usize, MemoryError> {
        if !self.fts {
            return Ok(0);
        }
        self.setup_fts(true).await
    }

    /// The `MATCH` expression for a prompt: its query tokens, the rarest 32 when
    /// there are more (by document frequency in the index). `None` when no
    /// token is usable.
    async fn match_expression(&self, text: &str) -> Result<Option<String>, MemoryError> {
        use crate::fts_query::{MAX_QUERY_TOKENS, match_expression, query_tokens, rarest};
        let tokens = query_tokens(text);
        if tokens.len() > MAX_QUERY_TOKENS {
            let mut frequency = Vec::with_capacity(tokens.len());
            for token in &tokens {
                let n: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM memories_fts WHERE memories_fts MATCH ?1",
                )
                .bind(format!("\"{token}\""))
                .fetch_one(&self.pool)
                .await
                .map_err(to_io_err)?;
                frequency.push(n as u64);
            }
            // A token no document holds matches nothing: it never takes a slot.
            let (tokens, frequency): (Vec<String>, Vec<u64>) = tokens
                .into_iter()
                .zip(frequency)
                .filter(|(_, n)| *n > 0)
                .unzip();
            return Ok(match_expression(&rarest(tokens, &frequency)));
        }
        Ok(match_expression(&tokens))
    }

    /// FTS5 recall: the prompt's word tokens (see `fts5_match_expression`) are
    /// matched in the index, the kind filter and `exclude` apply in the same
    /// query, and the best `query.limit` by `-bm25() x weight` come back (higher score is
    /// better) with no corpus cap. No usable token gives an empty result.
    pub async fn recall_fts5(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        if !self.fts {
            // No index to use (an encrypted store's inner database): rank in Rust.
            return Ok(self
                .recall_scored(query, exclude)
                .await?
                .into_iter()
                .map(|(score, memory)| ScoredMemory {
                    memory,
                    score: score as f32,
                })
                .collect());
        }
        let Some(expr) = self.match_expression(&query.text).await? else {
            return Ok(Vec::new());
        };
        let mut conditions = vec!["memories_fts MATCH ?1".to_owned()];
        let mut n = 1;
        if query.kind.is_some() {
            n += 1;
            conditions.push(format!("m.kind = ?{n}"));
        }
        if !exclude.is_empty() {
            let marks: Vec<String> = exclude
                .iter()
                .map(|_| {
                    n += 1;
                    format!("?{n}")
                })
                .collect();
            conditions.push(format!("m.kind NOT IN ({})", marks.join(", ")));
        }
        let sql = format!(
            "SELECT m.id, m.content, m.tags, m.weight, m.created_at, m.decay_at, m.kind,
                    m.description, m.embedding, -bm25(memories_fts) * m.weight AS score
             FROM memories_fts
             JOIN memories_fts_map mp ON mp.docid = memories_fts.rowid
             JOIN memories m ON m.id = mp.id
             WHERE {}
             ORDER BY score DESC, m.created_at DESC
             LIMIT ?{}",
            conditions.join(" AND "),
            n + 1
        );
        let mut q = sqlx::query(&sql).bind(expr);
        if let Some(kind) = query.kind {
            q = q.bind(kind_to_str(kind));
        }
        for kind in exclude {
            q = q.bind(kind_to_str(*kind));
        }
        let rows = q
            .bind(query.limit.min(i64::MAX as usize) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?;
        rows.iter()
            .map(|row| {
                let score: f64 = row.try_get("score").map_err(to_io_err)?;
                Ok(ScoredMemory {
                    memory: row_to_memory(row)?,
                    score: score as f32,
                })
            })
            .collect()
    }
}

impl SqliteStore {
    /// `recall` / `recall_excluding`. For a non-empty query the corpus is the
    /// most recent `MAX_RECALL_CORPUS` rows that pass the kind filter and the
    /// exclusions (both applied in SQL, before the cap); BM25 ranks that corpus
    /// and the best `query.limit` are returned. An empty query enumerates the
    /// newest `query.limit` rows with no ranking.
    async fn recall_with_exclusions(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<Memory>, MemoryError> {
        Ok(self
            .recall_scored(query, exclude)
            .await?
            .into_iter()
            .map(|(_, m)| m)
            .collect())
    }

    /// [`recall_with_exclusions`](Self::recall_with_exclusions) with each
    /// result's BM25 x weight score (`0.0` for an enumeration).
    async fn recall_scored(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<(f64, Memory)>, MemoryError> {
        let fetch_limit = if query.text.is_empty() {
            query.limit
        } else {
            MAX_RECALL_CORPUS
        };

        // Only placeholders are built into the SQL; every value is bound.
        let mut conditions: Vec<String> = Vec::new();
        let mut n = 0;
        if query.kind.is_some() {
            n += 1;
            conditions.push(format!("kind = ?{n}"));
        }
        if !exclude.is_empty() {
            let marks: Vec<String> = exclude
                .iter()
                .map(|_| {
                    n += 1;
                    format!("?{n}")
                })
                .collect();
            conditions.push(format!("kind NOT IN ({})", marks.join(", ")));
        }
        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        // The BM25 corpus does not need the (large) embedding column; an
        // enumeration returns the memories themselves, embedding included.
        let columns = if query.text.is_empty() {
            "id, content, tags, weight, created_at, decay_at, kind, description, embedding"
        } else {
            "id, content, tags, weight, created_at, decay_at, kind, description"
        };
        let sql = format!(
            "SELECT {columns}
             FROM memories
             {where_clause}
             ORDER BY created_at DESC
             LIMIT ?{}",
            n + 1
        );
        let mut q = sqlx::query(&sql);
        if let Some(kind) = query.kind {
            q = q.bind(kind_to_str(kind));
        }
        for kind in exclude {
            q = q.bind(kind_to_str(*kind));
        }
        let rows = q
            .bind(fetch_limit.min(i64::MAX as usize) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?;

        let memories: Vec<Memory> = rows
            .iter()
            .filter_map(|row| match row_to_memory(row) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(error = ?e, "skipping memory row that failed to deserialize");
                    None
                }
            })
            .collect();

        // Empty-query short-circuit: return all memories ordered by weight.
        if query.text.is_empty() {
            let mut all: Vec<Memory> = memories;
            all.sort_by(|a, b| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let results = all
                .into_iter()
                .take(query.limit)
                .map(|m| (0.0, m))
                .collect();
            return Ok(results);
        }

        let mut results = bm25_scored(memories, &query.text, query.limit);

        // The corpus was read without the embedding column; load the vectors of
        // the memories actually returned, in one bound query.
        if !results.is_empty() {
            let marks: Vec<String> = (1..=results.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT id, embedding FROM memories WHERE id IN ({})",
                marks.join(", ")
            );
            let mut q = sqlx::query(&sql);
            for (_, m) in &results {
                q = q.bind(&m.id);
            }
            let rows = q.fetch_all(&self.pool).await.map_err(to_io_err)?;
            let mut by_id: std::collections::HashMap<String, Vec<f32>> = rows
                .iter()
                .filter_map(|row| {
                    let id: String = row.try_get("id").ok()?;
                    let bytes: Vec<u8> = row.try_get::<Option<Vec<u8>>, _>("embedding").ok()??;
                    Some((id, bytes_to_embedding(&bytes)))
                })
                .collect();
            for (_, m) in &mut results {
                m.embedding = by_id.remove(&m.id);
            }
        }

        Ok(results)
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        let tags_json = serde_json::to_string(&memory.tags)?;
        let created_at = memory.created_at as i64;
        let decay_at = memory.decay_at.map(|t| t as i64);
        let kind_str = kind_to_str(memory.kind);
        let embedding_bytes = memory.embedding.as_deref().map(embedding_to_bytes);

        sqlx::query(
            "INSERT OR REPLACE INTO memories
             (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&memory.id)
        .bind(&memory.content)
        .bind(&tags_json)
        .bind(memory.weight)
        .bind(created_at)
        .bind(decay_at)
        .bind(kind_str)
        .bind(&memory.description)
        .bind(embedding_bytes)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?;

        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        self.recall_with_exclusions(query, &[]).await
    }

    async fn recall_excluding(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<Memory>, MemoryError> {
        self.recall_with_exclusions(query, exclude).await
    }

    async fn recall_indexed(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Option<Vec<ScoredMemory>>, MemoryError> {
        self.recall_fts5(query, exclude).await.map(Some)
    }

    async fn reindex(&self) -> Result<Option<usize>, MemoryError> {
        self.reindex_fts().await.map(Some)
    }

    async fn encrypted_rows_sample(&self, limit: usize) -> Result<Vec<Memory>, MemoryError> {
        let rows = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description, embedding
             FROM memories
             WHERE (content LIKE 'enc:v1:%' OR content LIKE 'enc:v2:%') AND id != ?1
             LIMIT ?2",
        )
        .bind(SALT_SENTINEL_ID)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(to_io_err)?;
        rows.iter().map(row_to_memory).collect()
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        let row = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description, embedding
             FROM memories
             WHERE id = ?1
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_io_err)?;

        row.as_ref().map(row_to_memory_with).transpose()
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let rows_affected = match filter {
            MemoryFilter::Tag(tag) => {
                // Tags are stored as a JSON array, e.g. ["project:foo","bar"].
                // Match the quoted tag string anywhere inside that array.
                let pattern = format!("%\"{}\"%", tag);
                sqlx::query("DELETE FROM memories WHERE tags LIKE ?1 AND id != ?2")
                    .bind(&pattern)
                    .bind(SALT_SENTINEL_ID)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::Before(ts) => {
                let ts = *ts as i64;
                sqlx::query("DELETE FROM memories WHERE created_at < ?1 AND id != ?2")
                    .bind(ts)
                    .bind(SALT_SENTINEL_ID)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::After(ts) => {
                let ts = *ts as i64;
                sqlx::query("DELETE FROM memories WHERE created_at > ?1 AND id != ?2")
                    .bind(ts)
                    .bind(SALT_SENTINEL_ID)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::Kind(kind) => {
                let kind_str = kind_to_str(*kind);
                sqlx::query("DELETE FROM memories WHERE kind = ?1 AND id != ?2")
                    .bind(kind_str)
                    .bind(SALT_SENTINEL_ID)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
        };

        Ok(rows_affected as u32)
    }

    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        // Multiply all weights by factor.
        sqlx::query("UPDATE memories SET weight = weight * ?1")
            .bind(factor)
            .execute(&self.pool)
            .await
            .map_err(to_io_err)?;

        // Remove memories whose decay_at has been reached.
        let now = now as i64;
        let rows_affected = sqlx::query(
            "DELETE FROM memories WHERE decay_at IS NOT NULL AND decay_at <= ?1 AND id != ?2",
        )
        .bind(now)
        .bind(SALT_SENTINEL_ID)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?
        .rows_affected();

        Ok(rows_affected as u32)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let rows = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description, embedding
             FROM memories",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_io_err)?;

        let memories: Result<Vec<Memory>, MemoryError> = rows.iter().map(row_to_memory).collect();
        let memories = memories?;

        let json = serde_json::to_string_pretty(&memories)?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = tokio::fs::read(path).await?;
        let memories: Vec<Memory> = serde_json::from_slice(&bytes)?;

        let store = SqliteStore::in_memory().await?;
        for memory in memories {
            store.record(memory).await?;
        }

        Ok(store)
    }

    async fn recall_semantic(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        let rows = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description, embedding
             FROM memories
             WHERE embedding IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_io_err)?;

        let mut scored: Vec<ScoredMemory> = rows
            .iter()
            .filter_map(|row| match row_to_memory(row) {
                Ok(m) => {
                    let embedding = m.embedding.clone()?;
                    let score = cosine_similarity(query_embedding, &embedding);
                    Some(ScoredMemory { memory: m, score })
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "skipping memory row that failed to deserialize");
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }
}

#[async_trait]
impl SealedVectorStore for SqliteStore {
    async fn record_if_absent(&self, memory: Memory) -> Result<bool, MemoryError> {
        let tags_json = serde_json::to_string(&memory.tags)?;
        let embedding_bytes = memory.embedding.as_deref().map(embedding_to_bytes);
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO memories
             (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&memory.id)
        .bind(&memory.content)
        .bind(&tags_json)
        .bind(memory.weight)
        .bind(memory.created_at as i64)
        .bind(memory.decay_at.map(|t| t as i64))
        .bind(kind_to_str(memory.kind))
        .bind(&memory.description)
        .bind(embedding_bytes)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?
        .rows_affected();
        Ok(inserted > 0)
    }

    async fn has_encrypted_rows(&self) -> Result<bool, MemoryError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM memories
             WHERE (content LIKE 'enc:v1:%' OR content LIKE 'enc:v2:%') AND id != ?1 LIMIT 1",
        )
        .bind(SALT_SENTINEL_ID)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_io_err)?;
        Ok(found.is_some())
    }

    async fn set_description_if_absent(
        &self,
        id: &str,
        description: &str,
    ) -> Result<bool, MemoryError> {
        let done = sqlx::query(
            "UPDATE memories SET description = ?1 WHERE id = ?2 AND description IS NULL",
        )
        .bind(description)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?;
        Ok(done.rows_affected() == 1)
    }

    async fn sealed_vector(&self, id: &str) -> Result<Option<Vec<u8>>, MemoryError> {
        let row = sqlx::query("SELECT blob FROM memory_vectors_sealed WHERE id = ?1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await;
        match row {
            Ok(r) => Ok(r.and_then(|r| r.try_get::<Vec<u8>, _>("blob").ok())),
            // No side table yet: nothing sealed.
            Err(e) if e.to_string().contains("no such table") => Ok(None),
            Err(e) => Err(to_io_err(e)),
        }
    }

    async fn vector_migration_candidates(&self) -> Result<Vec<(String, Vec<u8>)>, MemoryError> {
        let rows = sqlx::query(
            "SELECT id, embedding FROM memories
             WHERE content LIKE 'enc:v1:%' AND embedding IS NOT NULL
             ORDER BY id LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_io_err)?;
        Ok(rows
            .iter()
            .filter_map(|r| Some((r.try_get("id").ok()?, r.try_get("embedding").ok()?)))
            .collect())
    }

    async fn move_vector_to_sealed(&self, id: &str, blob: &[u8]) -> Result<(), MemoryError> {
        self.ensure_sealed_table().await?;
        let mut tx = self.pool.begin().await.map_err(to_io_err)?;
        sqlx::query("INSERT OR REPLACE INTO memory_vectors_sealed (id, blob) VALUES (?1, ?2)")
            .bind(id)
            .bind(blob)
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        sqlx::query("UPDATE memories SET embedding = NULL WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(to_io_err)?;
        tx.commit().await.map_err(to_io_err)
    }

    async fn move_vectors_to_sealed(&self, moves: &[(String, Vec<u8>)]) -> Result<(), MemoryError> {
        if moves.is_empty() {
            return Ok(());
        }
        self.ensure_sealed_table().await?;
        let mut tx = self.pool.begin().await.map_err(to_io_err)?;
        for (id, blob) in moves {
            sqlx::query("INSERT OR REPLACE INTO memory_vectors_sealed (id, blob) VALUES (?1, ?2)")
                .bind(id)
                .bind(blob)
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
            sqlx::query("UPDATE memories SET embedding = NULL WHERE id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
        }
        tx.commit().await.map_err(to_io_err)
    }

    async fn record_sealed(
        &self,
        memory: Memory,
        sealed: Option<Vec<u8>>,
    ) -> Result<(), MemoryError> {
        if sealed.is_some() {
            self.ensure_sealed_table().await?;
        }
        let tags_json = serde_json::to_string(&memory.tags)?;
        let embedding_bytes = memory.embedding.as_deref().map(embedding_to_bytes);
        let mut tx = self.pool.begin().await.map_err(to_io_err)?;
        sqlx::query(
            "INSERT OR REPLACE INTO memories
             (id, content, tags, weight, created_at, decay_at, kind, description, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&memory.id)
        .bind(&memory.content)
        .bind(&tags_json)
        .bind(memory.weight)
        .bind(memory.created_at as i64)
        .bind(memory.decay_at.map(|t| t as i64))
        .bind(kind_to_str(memory.kind))
        .bind(&memory.description)
        .bind(embedding_bytes)
        .execute(&mut *tx)
        .await
        .map_err(to_io_err)?;
        match &sealed {
            Some(blob) => {
                sqlx::query(
                    "INSERT OR REPLACE INTO memory_vectors_sealed (id, blob) VALUES (?1, ?2)",
                )
                .bind(&memory.id)
                .bind(blob)
                .execute(&mut *tx)
                .await
                .map_err(to_io_err)?;
            }
            None => {
                match sqlx::query("DELETE FROM memory_vectors_sealed WHERE id = ?1")
                    .bind(&memory.id)
                    .execute(&mut *tx)
                    .await
                {
                    Ok(_) => {}
                    // No side table yet: nothing sealed to clear.
                    Err(e) if e.to_string().contains("no such table") => {}
                    Err(e) => return Err(to_io_err(e)),
                }
            }
        }
        tx.commit().await.map_err(to_io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_memory(id: &str, content: &str) -> Memory {
        Memory {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: None,
        }
    }

    fn make_memory_with_embedding(id: &str, content: &str, emb: Vec<f32>) -> Memory {
        Memory {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: Some(emb),
        }
    }

    /// Writes made through one `SqliteStore` instance must be visible when the
    /// same database file is opened by a second `SqliteStore` instance.  This
    /// verifies that the persistent store is actually persisting.
    #[tokio::test]
    async fn writes_persist_across_instances() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("memory.db");

        // First instance: record a memory.
        {
            let store = SqliteStore::open(&db_path)
                .await
                .expect("open first instance");
            store
                .record(make_memory("id-1", "persistent content"))
                .await
                .expect("record");
            // store is dropped here, closing the pool.
        }

        // Second instance: open the same path and recall.
        {
            let store = SqliteStore::open(&db_path)
                .await
                .expect("open second instance");
            let results = store
                .recall(&MemoryQuery {
                    text: "persistent".to_owned(),
                    limit: 10,
                    kind: None,
                })
                .await
                .expect("recall");

            assert_eq!(results.len(), 1, "expected 1 memory after re-opening DB");
            assert_eq!(results[0].id, "id-1");
            assert_eq!(results[0].content, "persistent content");
        }
    }

    /// An empty query should return all stored memories (up to limit).
    #[tokio::test]
    async fn empty_query_returns_all() {
        let store = SqliteStore::in_memory().await.expect("in_memory");
        store
            .record(make_memory("a", "alpha"))
            .await
            .expect("record a");
        store
            .record(make_memory("b", "beta"))
            .await
            .expect("record b");

        let results = store
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 100,
                kind: None,
            })
            .await
            .expect("recall");

        assert_eq!(results.len(), 2, "expected 2 memories from empty query");
    }

    // -----------------------------------------------------------------------
    // Semantic recall tests (red phase — require embedding column + impl)
    // -----------------------------------------------------------------------

    /// A memory stored with an embedding must come back with the same embedding
    /// (within f32 round-trip tolerance from the BLOB encoding).
    #[tokio::test]
    async fn sqlite_store_record_with_embedding_round_trips() {
        let store = SqliteStore::in_memory().await.expect("in_memory");
        let emb = vec![0.1_f32, 0.2, 0.3];
        store
            .record(make_memory_with_embedding(
                "id-emb",
                "content with embedding",
                emb.clone(),
            ))
            .await
            .expect("record");

        let result = store
            .recall_by_id("id-emb")
            .await
            .expect("recall_by_id")
            .expect("memory should exist");

        let returned = result.embedding.expect("embedding should be Some");
        assert_eq!(
            returned.len(),
            emb.len(),
            "embedding length must match after round-trip"
        );
        for (got, expected) in returned.iter().zip(emb.iter()) {
            assert!(
                (got - expected).abs() < 1e-5,
                "embedding element mismatch: got {got}, expected {expected}"
            );
        }
    }

    /// A memory stored without an embedding must come back with embedding=None.
    #[tokio::test]
    async fn sqlite_store_record_without_embedding_round_trips() {
        let store = SqliteStore::in_memory().await.expect("in_memory");
        store
            .record(make_memory("id-no-emb", "content without embedding"))
            .await
            .expect("record");

        let result = store
            .recall_by_id("id-no-emb")
            .await
            .expect("recall_by_id")
            .expect("memory should exist");

        assert!(
            result.embedding.is_none(),
            "embedding should be None when not stored"
        );
    }

    /// recall_semantic returns the top-K memories ranked by cosine similarity.
    #[tokio::test]
    async fn sqlite_store_recall_semantic_returns_top_k() {
        let store = SqliteStore::in_memory().await.expect("in_memory");

        // Three orthogonal 4-dim unit vectors.
        store
            .record(make_memory_with_embedding(
                "alpha",
                "alpha content",
                vec![1.0, 0.0, 0.0, 0.0],
            ))
            .await
            .expect("record alpha");
        store
            .record(make_memory_with_embedding(
                "beta",
                "beta content",
                vec![0.0, 1.0, 0.0, 0.0],
            ))
            .await
            .expect("record beta");
        store
            .record(make_memory_with_embedding(
                "gamma",
                "gamma content",
                vec![0.0, 0.0, 1.0, 0.0],
            ))
            .await
            .expect("record gamma");

        // Query aligned with "alpha".
        let query = vec![1.0_f32, 0.0, 0.0, 0.0];
        let results = store
            .recall_semantic(&query, 1)
            .await
            .expect("recall_semantic");

        assert_eq!(results.len(), 1, "expected exactly 1 result");
        assert_eq!(
            results[0].memory.id, "alpha",
            "top result should be 'alpha'"
        );
        assert!(
            (results[0].score - 1.0).abs() < 1e-5,
            "score for identical direction should be 1.0, got {}",
            results[0].score
        );
    }

    /// recall_semantic must silently skip memories that have no stored embedding.
    #[tokio::test]
    async fn sqlite_store_recall_semantic_skips_null_embeddings() {
        let store = SqliteStore::in_memory().await.expect("in_memory");

        store
            .record(make_memory_with_embedding(
                "with-emb",
                "has embedding",
                vec![1.0, 0.0],
            ))
            .await
            .expect("record with embedding");
        store
            .record(make_memory("without-emb", "no embedding"))
            .await
            .expect("record without embedding");

        let query = vec![1.0_f32, 0.0];
        let results = store
            .recall_semantic(&query, 10)
            .await
            .expect("recall_semantic");

        assert_eq!(
            results.len(),
            1,
            "only memory with embedding should be returned"
        );
        assert_eq!(results[0].memory.id, "with-emb");
    }

    /// Opening the same SQLite file twice must not fail — the schema migration
    /// (ADD COLUMN embedding BLOB) must be idempotent.
    #[tokio::test]
    async fn sqlite_store_schema_migration_idempotent() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("idem.db");

        let store1 = SqliteStore::open(&db_path)
            .await
            .expect("first open should succeed");
        drop(store1);

        // Second open must not error out even though embedding column already exists.
        let store2 = SqliteStore::open(&db_path)
            .await
            .expect("second open (idempotent migration) should succeed");
        drop(store2);
    }
}
