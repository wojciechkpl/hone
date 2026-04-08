use std::time::{SystemTime, UNIX_EPOCH};

use hone_proto::SessionId;

use crate::error::ServerError;
use crate::session::{SessionInfo, SessionStore};
use crate::sqlite_session::SqliteSessionStore;

/// Runtime-dispatched session backend.
///
/// `InMemory` is used when no `HONE_SESSION_DB` env var is set.
/// `Sqlite` is used when the env var points to a database file.
pub enum SessionStorage {
    InMemory(SessionStore),
    Sqlite(SqliteSessionStore),
}

impl SessionStorage {
    /// Create a brand-new session, persist it, and return its `SessionInfo`.
    pub async fn create_new(&self) -> Result<SessionInfo, ServerError> {
        match self {
            Self::InMemory(s) => Ok(s.create().await),
            Self::Sqlite(s) => {
                let info = SessionInfo {
                    id: SessionId::new(),
                    created_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                };
                s.create(info.clone()).await?;
                Ok(info)
            }
        }
    }

    pub async fn get(&self, id: &str) -> Result<Option<SessionInfo>, ServerError> {
        match self {
            Self::InMemory(s) => Ok(s.get(id).await),
            Self::Sqlite(s) => s.get(id).await,
        }
    }

    pub async fn delete(&self, id: &str) -> Result<bool, ServerError> {
        match self {
            Self::InMemory(s) => Ok(s.delete(id).await),
            Self::Sqlite(s) => s.delete(id).await,
        }
    }

    pub async fn list(&self) -> Result<Vec<SessionInfo>, ServerError> {
        match self {
            Self::InMemory(s) => Ok(s.list().await),
            Self::Sqlite(s) => s.list().await,
        }
    }

    /// Remove sessions whose `created_at` is strictly less than `cutoff` (Unix seconds).
    /// Returns the number of sessions removed.
    pub async fn gc(&self, cutoff: u64) -> Result<u32, ServerError> {
        match self {
            Self::InMemory(s) => Ok(s.gc(cutoff).await),
            Self::Sqlite(s) => s.gc(cutoff).await,
        }
    }
}
