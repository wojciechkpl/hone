use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

use hone_proto::SessionId;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionInfo {
    #[schema(value_type = String, format = "uuid")]
    pub id: SessionId,
    pub created_at: u64,
}

#[derive(Clone)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionInfo>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create(&self) -> SessionInfo {
        let id = SessionId::new();
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let info = SessionInfo {
            id: id.clone(),
            created_at,
        };

        self.sessions
            .write()
            .await
            .insert(id.to_string(), info.clone());

        info
    }

    pub async fn get(&self, id: &str) -> Option<SessionInfo> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn delete(&self, id: &str) -> bool {
        self.sessions.write().await.remove(id).is_some()
    }

    pub async fn list(&self) -> Vec<SessionInfo> {
        self.sessions.read().await.values().cloned().collect()
    }

    /// Remove all sessions whose `created_at` is strictly less than `cutoff` (Unix seconds).
    /// Returns the number of sessions removed.
    pub async fn gc(&self, cutoff: u64) -> u32 {
        let mut guard = self.sessions.write().await;
        let before = guard.len();
        guard.retain(|_, info| info.created_at >= cutoff);
        (before - guard.len()) as u32
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}
