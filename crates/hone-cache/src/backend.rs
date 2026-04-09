use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::key::{CacheKey, InvalidationTag};

/// Errors produced by cache backends.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("backend error: {0}")]
    Backend(String),
}

/// A single cached value with its metadata.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub bytes: Arc<[u8]>,
    pub created_at: i64,
    pub last_accessed: i64,
    pub hit_count: u32,
    pub size_bytes: u32,
    pub invalidator: InvalidationTag,
}

impl CacheEntry {
    pub fn new(bytes: Vec<u8>, invalidator: InvalidationTag) -> Self {
        let size = bytes.len() as u32;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            bytes: Arc::from(bytes.into_boxed_slice()),
            created_at: now,
            last_accessed: now,
            hit_count: 0,
            size_bytes: size,
            invalidator,
        }
    }
}

/// Aggregate statistics for a backend instance.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub bytes_stored: u64,
    pub entry_count: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }
}

/// Storage backend for the cache. Implementors must be `Send + Sync` so they
/// can be wrapped in `Arc<dyn CacheBackend>` and shared across tasks.
#[async_trait]
pub trait CacheBackend: Send + Sync {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError>;
    async fn put(&self, key: &CacheKey, entry: CacheEntry) -> Result<(), CacheError>;
    async fn delete(&self, key: &CacheKey) -> Result<(), CacheError>;
    /// Remove all entries whose namespace matches `namespace`.
    /// Returns the number of entries removed.
    async fn invalidate_namespace(&self, namespace: &str) -> Result<u64, CacheError>;
    async fn stats(&self) -> CacheStats;
}
