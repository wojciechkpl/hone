use std::path::Path;

use async_trait::async_trait;
use sqlx::Row;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};

pub struct SqliteStore {
    pool: sqlx::SqlitePool,
}

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

/// Probe `PRAGMA table_info(memories)` and return the set of column names
/// present in the table.
async fn existing_columns(pool: &sqlx::SqlitePool) -> Result<Vec<String>, MemoryError> {
    let rows = sqlx::query("PRAGMA table_info(memories)")
        .fetch_all(pool)
        .await
        .map_err(to_io_err)?;

    let names = rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("name").ok())
        .collect();

    Ok(names)
}

/// Run idempotent `ALTER TABLE` migrations for any columns that are missing.
async fn migrate_schema(pool: &sqlx::SqlitePool) -> Result<(), MemoryError> {
    let cols = existing_columns(pool).await?;

    // Two processes opening the same DB simultaneously can both observe the
    // column missing and race to ALTER; the second hit is a no-op, not an error.
    if !cols.iter().any(|c| c == "kind") {
        match sqlx::query(
            "ALTER TABLE memories ADD COLUMN kind TEXT NOT NULL DEFAULT 'conversation'",
        )
        .execute(pool)
        .await
        {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(to_io_err(e)),
        }
    }

    if !cols.iter().any(|c| c == "description") {
        match sqlx::query("ALTER TABLE memories ADD COLUMN description TEXT")
            .execute(pool)
            .await
        {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column name") => {}
            Err(e) => return Err(to_io_err(e)),
        }
    }

    Ok(())
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
        migrate_schema(&pool).await?;
        sqlx::query(CREATE_KIND_INDEX_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        sqlx::query(CREATE_META_TABLE_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;

        let mut store = Self { pool };

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
        // new columns — but migrate_schema is called anyway for consistency.
        migrate_schema(&pool).await?;
        sqlx::query(CREATE_KIND_INDEX_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn record(&mut self, memory: Memory) -> Result<(), MemoryError> {
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
        let query_lower = query.text.to_lowercase();

        // Fetch all rows and score in Rust — SQLite LIKE is case-insensitive for
        // ASCII but scoring with overlap count requires application-side logic that
        // mirrors InMemoryStore's behaviour.
        let rows = if query_lower.is_empty() {
            sqlx::query(
                "SELECT id, content, tags, weight, created_at, decay_at, kind, description
                 FROM memories",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?
        } else {
            let pattern = format!("%{}%", query_lower);
            sqlx::query(
                "SELECT id, content, tags, weight, created_at, decay_at, kind, description
                 FROM memories
                 WHERE lower(content) LIKE ?1",
            )
            .bind(&pattern)
            .fetch_all(&self.pool)
            .await
            .map_err(to_io_err)?
        };

        let mut scored: Vec<(f64, Memory)> = rows
            .iter()
            .filter_map(|row| {
                let memory = row_to_memory(row).ok()?;
                let score = relevance_score(&memory, &query_lower);
                if score > 0.0 {
                    Some((score * memory.weight, memory))
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

    async fn forget(&mut self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
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

    async fn decay(&mut self, factor: f64, now: u64) -> Result<u32, MemoryError> {
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
        std::fs::write(path, json)?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = std::fs::read(path)?;
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
            let mut store = SqliteStore::open(&db_path)
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
        let mut store = SqliteStore::in_memory().await.expect("in_memory");
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
            })
            .await
            .expect("recall");

        assert_eq!(results.len(), 2, "expected 2 memories from empty query");
    }
}

/// Count overlapping case-insensitive occurrences of `query_lower` in the
/// memory content.  Mirrors the scoring used by `InMemoryStore`.
fn relevance_score(memory: &Memory, query_lower: &str) -> f64 {
    if query_lower.is_empty() {
        return 1.0;
    }
    let content_lower = memory.content.to_lowercase();
    let mut count = 0usize;
    let mut start = 0;
    while let Some(pos) = content_lower[start..].find(query_lower) {
        count += 1;
        start += pos + 1;
    }
    count as f64
}
