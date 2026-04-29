use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::backend::{CacheBackend, CacheEntry, CacheError, CacheStats};
use crate::key::{CacheKey, InvalidationTag};

// Run each statement separately because sqlx does not support multi-statement
// queries through the standard `query` interface. WAL mode and other pragmas
// are issued as individual exec calls during `apply_schema`.
const CREATE_CACHE_ENTRIES: &str = "
CREATE TABLE IF NOT EXISTS cache_entries (
    namespace        TEXT    NOT NULL,
    hash_hex         TEXT    NOT NULL,
    bytes            BLOB    NOT NULL,
    created_at       INTEGER NOT NULL,
    last_accessed    INTEGER NOT NULL,
    hit_count        INTEGER NOT NULL DEFAULT 0,
    size_bytes       INTEGER NOT NULL,
    invalidator_json TEXT    NOT NULL,
    PRIMARY KEY (namespace, hash_hex)
)";

const CREATE_IDX_NAMESPACE: &str =
    "CREATE INDEX IF NOT EXISTS idx_cache_namespace ON cache_entries(namespace)";

const CREATE_IDX_LAST_ACCESSED: &str =
    "CREATE INDEX IF NOT EXISTS idx_cache_last_accessed ON cache_entries(last_accessed)";

const CREATE_CACHE_STATS: &str = "
CREATE TABLE IF NOT EXISTS cache_stats (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    hits           INTEGER NOT NULL DEFAULT 0,
    misses         INTEGER NOT NULL DEFAULT 0,
    invalidations  INTEGER NOT NULL DEFAULT 0
)";

const INSERT_STATS_ROW: &str = "INSERT OR IGNORE INTO cache_stats (id) VALUES (1)";

/// Persistent SQLite backend.  Uses runtime queries (no compile-time macros)
/// so the database does not need to exist at compile time.
pub struct SqliteBackend {
    pool: SqlitePool,
}

impl SqliteBackend {
    /// Open (or create) a file-backed SQLite cache at `path`.
    pub async fn open(path: &Path) -> Result<Self, CacheError> {
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let pool = SqlitePool::connect(&url)
            .await
            .map_err(|e| CacheError::Backend(format!("connect: {e}")))?;
        Self::apply_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Create an in-memory SQLite backend (useful in tests).
    pub async fn in_memory() -> Result<Self, CacheError> {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .map_err(|e| CacheError::Backend(format!("connect: {e}")))?;
        Self::apply_schema(&pool).await?;
        Ok(Self { pool })
    }

    async fn apply_schema(pool: &SqlitePool) -> Result<(), CacheError> {
        for stmt in [
            CREATE_CACHE_ENTRIES,
            CREATE_IDX_NAMESPACE,
            CREATE_IDX_LAST_ACCESSED,
            CREATE_CACHE_STATS,
            INSERT_STATS_ROW,
        ] {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .map_err(|e| CacheError::Backend(format!("schema: {e}")))?;
        }
        Ok(())
    }
}

#[async_trait]
impl CacheBackend for SqliteBackend {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let hex = key.hash_hex();

        let row: Option<(Vec<u8>, i64, i64, i32, i64, String)> = sqlx::query_as(
            "SELECT bytes, created_at, last_accessed, hit_count, size_bytes, invalidator_json
             FROM cache_entries
             WHERE namespace = ?1 AND hash_hex = ?2",
        )
        .bind(&key.namespace)
        .bind(&hex)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CacheError::Backend(e.to_string()))?;

        let Some((bytes, created_at, last_accessed, hit_count, size_bytes, inv_json)) = row else {
            sqlx::query("UPDATE cache_stats SET misses = misses + 1 WHERE id = 1")
                .execute(&self.pool)
                .await
                .map_err(|e| CacheError::Backend(e.to_string()))?;
            return Ok(None);
        };

        let invalidator: InvalidationTag = serde_json::from_str(&inv_json)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        if !invalidator.is_valid(now) {
            sqlx::query("DELETE FROM cache_entries WHERE namespace = ?1 AND hash_hex = ?2")
                .bind(&key.namespace)
                .bind(&hex)
                .execute(&self.pool)
                .await
                .map_err(|e| CacheError::Backend(e.to_string()))?;

            sqlx::query(
                "UPDATE cache_stats \
                 SET misses = misses + 1, invalidations = invalidations + 1 \
                 WHERE id = 1",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;

            return Ok(None);
        }

        sqlx::query("UPDATE cache_stats SET hits = hits + 1 WHERE id = 1")
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;

        Ok(Some(CacheEntry {
            bytes: Arc::from(bytes.into_boxed_slice()),
            created_at,
            last_accessed,
            hit_count: hit_count as u32,
            size_bytes: size_bytes as u32,
            invalidator,
        }))
    }

    async fn put(&self, key: &CacheKey, entry: CacheEntry) -> Result<(), CacheError> {
        let inv_json = serde_json::to_string(&entry.invalidator)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;
        let hex = key.hash_hex();
        let bytes: Vec<u8> = entry.bytes.to_vec();

        sqlx::query(
            "INSERT OR REPLACE INTO cache_entries
             (namespace, hash_hex, bytes, created_at, last_accessed,
              hit_count, size_bytes, invalidator_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&key.namespace)
        .bind(&hex)
        .bind(&bytes)
        .bind(entry.created_at)
        .bind(entry.last_accessed)
        .bind(entry.hit_count as i32)
        .bind(entry.size_bytes as i64)
        .bind(&inv_json)
        .execute(&self.pool)
        .await
        .map_err(|e| CacheError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, key: &CacheKey) -> Result<(), CacheError> {
        let hex = key.hash_hex();
        sqlx::query("DELETE FROM cache_entries WHERE namespace = ?1 AND hash_hex = ?2")
            .bind(&key.namespace)
            .bind(&hex)
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn invalidate_namespace(&self, namespace: &str) -> Result<u64, CacheError> {
        let result = sqlx::query("DELETE FROM cache_entries WHERE namespace = ?1")
            .bind(namespace)
            .execute(&self.pool)
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;

        let removed = result.rows_affected();
        if removed > 0 {
            sqlx::query("UPDATE cache_stats SET invalidations = invalidations + ?1 WHERE id = 1")
                .bind(removed as i64)
                .execute(&self.pool)
                .await
                .map_err(|e| CacheError::Backend(e.to_string()))?;
        }
        Ok(removed)
    }

    async fn stats(&self) -> CacheStats {
        let stats: Option<(i64, i64, i64)> =
            sqlx::query_as("SELECT hits, misses, invalidations FROM cache_stats WHERE id = 1")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();

        let bytes_and_count: Option<(Option<i64>, i64)> =
            sqlx::query_as("SELECT SUM(size_bytes), COUNT(*) FROM cache_entries")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();

        let (hits, misses, invalidations) = stats.unwrap_or((0, 0, 0));
        let (bytes_stored, entry_count) = bytes_and_count
            .map(|(b, c)| (b.unwrap_or(0) as u64, c as u64))
            .unwrap_or((0, 0));

        CacheStats {
            hits: hits as u64,
            misses: misses as u64,
            invalidations: invalidations as u64,
            bytes_stored,
            entry_count,
        }
    }
}
