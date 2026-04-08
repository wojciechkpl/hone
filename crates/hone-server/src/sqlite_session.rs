use std::path::Path;

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ServerError;
use crate::session::SessionInfo;

pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub async fn open(path: &Path) -> Result<Self, ServerError> {
        let url = format!(
            "sqlite://{}?mode=rwc",
            path.to_str().ok_or_else(|| ServerError::Internal(
                "session DB path is not valid UTF-8".to_owned()
            ))?
        );
        Self::connect(&url).await
    }

    pub async fn in_memory() -> Result<Self, ServerError> {
        Self::connect("sqlite::memory:").await
    }

    async fn connect(url: &str) -> Result<Self, ServerError> {
        let pool = SqlitePool::connect(url)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id         TEXT    PRIMARY KEY,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(|e| ServerError::Internal(e.to_string()))?;

        Ok(Self { pool })
    }

    pub async fn create(&self, info: SessionInfo) -> Result<(), ServerError> {
        sqlx::query("INSERT INTO sessions (id, created_at) VALUES (?1, ?2)")
            .bind(info.id.to_string())
            .bind(info.created_at as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<SessionInfo>, ServerError> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT id, created_at FROM sessions WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?;

        Ok(row
            .map(|(id, created_at)| {
                Uuid::parse_str(&id)
                    .map(|u| SessionInfo {
                        id: hone_proto::SessionId::from_uuid(u),
                        created_at: created_at as u64,
                    })
                    .map_err(|e| ServerError::Internal(format!("corrupt session id: {e}")))
            })
            .transpose()?)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ServerError> {
        let result = sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete sessions created before `cutoff` (Unix seconds).
    /// Returns the number of rows deleted.
    pub async fn gc(&self, cutoff: u64) -> Result<u32, ServerError> {
        let result = sqlx::query("DELETE FROM sessions WHERE created_at < ?1")
            .bind(cutoff as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| ServerError::Internal(e.to_string()))?;
        Ok(result.rows_affected() as u32)
    }

    pub async fn list(&self) -> Result<Vec<SessionInfo>, ServerError> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("SELECT id, created_at FROM sessions ORDER BY created_at ASC")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| ServerError::Internal(e.to_string()))?;

        rows.into_iter()
            .map(|(id, created_at)| {
                Uuid::parse_str(&id)
                    .map(|u| SessionInfo {
                        id: hone_proto::SessionId::from_uuid(u),
                        created_at: created_at as u64,
                    })
                    .map_err(|e| ServerError::Internal(format!("corrupt session id: {e}")))
            })
            .collect()
    }
}
