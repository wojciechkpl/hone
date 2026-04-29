use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub id: String,
    pub session_id: String,
    pub timestamp: u64,
    pub kind: AuditKind,
    pub actor: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditKind {
    ToolCall,
    ModelRequest,
    SessionStart,
    SessionEnd,
    PolicyViolation,
}

#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub session_id: Option<String>,
    pub kind: Option<AuditKind>,
    pub after: Option<u64>,
    pub before: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn record(&self, entry: AuditEntry) -> Result<(), AuditError>;
    async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>, AuditError>;
    async fn export(&self, path: &Path) -> Result<(), AuditError>;
    async fn count(&self) -> usize;
}

/// In-memory append-only audit log for testing and local use.
pub struct InMemoryAuditLog {
    entries: RwLock<Vec<AuditEntry>>,
}

impl InMemoryAuditLog {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditLog for InMemoryAuditLog {
    async fn record(&self, entry: AuditEntry) -> Result<(), AuditError> {
        self.entries.write().await.push(entry);
        Ok(())
    }

    async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>, AuditError> {
        let entries = self.entries.read().await;
        let results = entries
            .iter()
            .filter(|e| {
                if let Some(ref sid) = filter.session_id {
                    if &e.session_id != sid {
                        return false;
                    }
                }
                if let Some(kind) = filter.kind {
                    if e.kind != kind {
                        return false;
                    }
                }
                if let Some(after) = filter.after {
                    if e.timestamp <= after {
                        return false;
                    }
                }
                if let Some(before) = filter.before {
                    if e.timestamp >= before {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        Ok(results)
    }

    async fn export(&self, path: &Path) -> Result<(), AuditError> {
        let entries = self.entries.read().await;
        let json = serde_json::to_vec_pretty(&*entries)?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    async fn count(&self) -> usize {
        self.entries.read().await.len()
    }
}
