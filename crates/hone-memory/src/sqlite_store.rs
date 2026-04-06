use std::path::Path;

use async_trait::async_trait;
use sqlx::Row;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

pub struct SqliteStore {
    pool: sqlx::SqlitePool,
}

const CREATE_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS memories (
        id         TEXT    PRIMARY KEY,
        content    TEXT    NOT NULL,
        tags       TEXT    NOT NULL DEFAULT '[]',
        weight     REAL    NOT NULL DEFAULT 1.0,
        created_at INTEGER NOT NULL,
        decay_at   INTEGER
    )
";

fn to_io_err(e: impl std::fmt::Display) -> MemoryError {
    MemoryError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

fn row_to_memory(row: &sqlx::sqlite::SqliteRow) -> Result<Memory, MemoryError> {
    let id: String = row.try_get("id").map_err(to_io_err)?;
    let content: String = row.try_get("content").map_err(to_io_err)?;
    let tags_json: String = row.try_get("tags").map_err(to_io_err)?;
    let weight: f64 = row.try_get("weight").map_err(to_io_err)?;
    let created_at: i64 = row.try_get("created_at").map_err(to_io_err)?;
    let decay_at: Option<i64> = row.try_get("decay_at").map_err(to_io_err)?;

    let tags: Vec<String> = serde_json::from_str(&tags_json)?;

    Ok(Memory {
        id,
        content,
        tags,
        weight,
        created_at: created_at as u64,
        decay_at: decay_at.map(|t| t as u64),
    })
}

impl SqliteStore {
    /// Open or create a SQLite database at the given path.
    pub async fn open(path: &Path) -> Result<Self, MemoryError> {
        let url = format!("sqlite:{}?mode=rwc", path.display());
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .map_err(to_io_err)?;
        sqlx::query(CREATE_TABLE_SQL)
            .execute(&pool)
            .await
            .map_err(to_io_err)?;
        Ok(Self { pool })
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
        Ok(Self { pool })
    }
}

#[async_trait]
impl MemoryStore for SqliteStore {
    async fn record(&mut self, memory: Memory) -> Result<(), MemoryError> {
        let tags_json = serde_json::to_string(&memory.tags)?;
        let created_at = memory.created_at as i64;
        let decay_at = memory.decay_at.map(|t| t as i64);

        sqlx::query(
            "INSERT OR REPLACE INTO memories (id, content, tags, weight, created_at, decay_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&memory.id)
        .bind(&memory.content)
        .bind(&tags_json)
        .bind(memory.weight)
        .bind(created_at)
        .bind(decay_at)
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
            sqlx::query("SELECT id, content, tags, weight, created_at, decay_at FROM memories")
                .fetch_all(&self.pool)
                .await
                .map_err(to_io_err)?
        } else {
            let pattern = format!("%{}%", query_lower);
            sqlx::query(
                "SELECT id, content, tags, weight, created_at, decay_at FROM memories
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
                let pattern = format!("%\"{}\"%" , tag);
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
        let rows_affected = sqlx::query(
            "DELETE FROM memories WHERE decay_at IS NOT NULL AND decay_at <= ?1",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(to_io_err)?
        .rows_affected();

        Ok(rows_affected as u32)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let rows = sqlx::query(
            "SELECT id, content, tags, weight, created_at, decay_at FROM memories",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(to_io_err)?;

        let memories: Result<Vec<Memory>, MemoryError> =
            rows.iter().map(|r| row_to_memory(r)).collect();
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

            sqlx::query(
                "INSERT OR REPLACE INTO memories (id, content, tags, weight, created_at, decay_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&memory.id)
            .bind(&memory.content)
            .bind(&tags_json)
            .bind(memory.weight)
            .bind(created_at)
            .bind(decay_at)
            .execute(&store.pool)
            .await
            .map_err(to_io_err)?;
        }

        Ok(store)
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
