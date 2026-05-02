use std::path::Path;

use async_trait::async_trait;
use sqlx::Row;

use crate::bm25::{Bm25Index, Bm25Params};
use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};

pub struct SqliteStore {
    pool: sqlx::SqlitePool,
}

/// The highest schema version this binary understands.  Bump this whenever a
/// new migration is added to `run_migrations`.
// crate-external for tests; not stable API
pub const CURRENT_SCHEMA_VERSION: i32 = 2;

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

    Ok(Memory {
        id,
        content,
        tags,
        weight,
        created_at: created_at as u64,
        decay_at: decay_at.map(|t| t as u64),
        kind,
        description,
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

        let store = Self { pool };

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
        Ok(Self { pool })
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        let tags_json = serde_json::to_string(&memory.tags)?;
        let created_at = memory.created_at as i64;
        let decay_at = memory.decay_at.map(|t| t as i64);
        let kind_str = kind_to_str(memory.kind);

        sqlx::query(
            "INSERT OR REPLACE INTO memories
             (id, content, tags, weight, created_at, decay_at, kind, description)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&memory.id)
        .bind(&memory.content)
        .bind(&tags_json)
        .bind(memory.weight)
        .bind(created_at)
        .bind(decay_at)
        .bind(kind_str)
        .bind(&memory.description)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?;

        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        // Fetch memories from the database — BM25 scoring happens in Rust.
        // For non-empty queries we cap at MAX_RECALL_CORPUS most-recent rows to
        // bound memory and CPU usage; see the const-level doc comment for the
        // IDF tradeoff.  For empty-query enumeration (e.g. render_index) the cap
        // has no purpose — BM25 is skipped entirely — so we honour query.limit
        // directly.
        //
        // When query.kind is Some(k), we add a WHERE kind = ? clause so the kind
        // filter is applied in the database BEFORE the LIMIT (pre-limit filter).
        let effective_limit = if query.text.is_empty() {
            query.limit
        } else {
            query.limit.min(MAX_RECALL_CORPUS)
        };

        let memories: Vec<Memory> = if let Some(kind) = query.kind {
            let kind_str = kind_to_str(kind);
            let rows = sqlx::query(
                "SELECT id, content, tags, weight, created_at, decay_at, kind, description
                 FROM memories
                 WHERE kind = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            .bind(kind_str)
            .bind(effective_limit.min(i64::MAX as usize) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?;

            rows.iter()
                .filter_map(|row| match row_to_memory(row) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::warn!(error = ?e, "skipping memory row that failed to deserialize");
                        None
                    }
                })
                .collect()
        } else {
            let rows = sqlx::query(
                "SELECT id, content, tags, weight, created_at, decay_at, kind, description
                 FROM memories
                 ORDER BY created_at DESC
                 LIMIT ?",
            )
            .bind(effective_limit.min(i64::MAX as usize) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?;

            rows.iter()
                .filter_map(|row| match row_to_memory(row) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        tracing::warn!(error = ?e, "skipping memory row that failed to deserialize");
                        None
                    }
                })
                .collect()
        };

        // Empty-query short-circuit: return all memories ordered by weight.
        if query.text.is_empty() {
            let mut all: Vec<Memory> = memories;
            all.sort_by(|a, b| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let results = all.into_iter().take(query.limit).collect();
            return Ok(results);
        }

        // Build a BM25 index over the searchable text of every memory.
        // Searchable text = description (if present) + "\n" + content.
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
                let bm25 = index.score(i, &query.text);
                if bm25 > 0.0 {
                    Some((bm25 * memory.weight, memory))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        let results = scored
            .into_iter()
            .take(query.limit)
            .map(|(_, m)| m)
            .collect();

        Ok(results)
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        let row = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description
             FROM memories
             WHERE id = ?1
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(to_io_err)?;

        row.as_ref().map(row_to_memory).transpose()
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let rows_affected = match filter {
            MemoryFilter::Tag(tag) => {
                // Tags are stored as a JSON array, e.g. ["project:foo","bar"].
                // Match the quoted tag string anywhere inside that array.
                let pattern = format!("%\"{}\"%", tag);
                sqlx::query("DELETE FROM memories WHERE tags LIKE ?1")
                    .bind(&pattern)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::Before(ts) => {
                let ts = *ts as i64;
                sqlx::query("DELETE FROM memories WHERE created_at < ?1")
                    .bind(ts)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::After(ts) => {
                let ts = *ts as i64;
                sqlx::query("DELETE FROM memories WHERE created_at > ?1")
                    .bind(ts)
                    .execute(&self.pool)
                    .await
                    .map_err(to_io_err)?
                    .rows_affected()
            }
            MemoryFilter::Kind(kind) => {
                let kind_str = kind_to_str(*kind);
                sqlx::query("DELETE FROM memories WHERE kind = ?1")
                    .bind(kind_str)
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
        let rows_affected =
            sqlx::query("DELETE FROM memories WHERE decay_at IS NOT NULL AND decay_at <= ?1")
                .bind(now)
                .execute(&self.pool)
                .await
                .map_err(to_io_err)?
                .rows_affected();

        Ok(rows_affected as u32)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let rows = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at, kind, description
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
            let tags_json = serde_json::to_string(&memory.tags)?;
            let created_at = memory.created_at as i64;
            let decay_at = memory.decay_at.map(|t| t as i64);
            let kind_str = kind_to_str(memory.kind);

            sqlx::query(
                "INSERT OR REPLACE INTO memories
                 (id, content, tags, weight, created_at, decay_at, kind, description)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(&memory.id)
            .bind(&memory.content)
            .bind(&tags_json)
            .bind(memory.weight)
            .bind(created_at)
            .bind(decay_at)
            .bind(kind_str)
            .bind(&memory.description)
            .execute(&store.pool)
            .await
            .map_err(to_io_err)?;
        }

        Ok(store)
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
}
