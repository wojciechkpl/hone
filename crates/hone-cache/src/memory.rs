use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::backend::{CacheBackend, CacheEntry, CacheError, CacheStats};
use crate::key::CacheKey;

/// In-process memory backend. Intended for tests and local dev runs where
/// persistence is not required.
pub struct InMemoryBackend {
    inner: Arc<Mutex<InMemoryInner>>,
}

struct InMemoryInner {
    entries: HashMap<Vec<u8>, (CacheKey, CacheEntry)>,
    stats: CacheStats,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InMemoryInner {
                entries: HashMap::new(),
                stats: CacheStats::default(),
            })),
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable composite key: namespace bytes + NUL separator + 32-byte hash.
fn key_bytes(key: &CacheKey) -> Vec<u8> {
    let mut buf = key.namespace.as_bytes().to_vec();
    buf.push(0);
    buf.extend_from_slice(&key.hash);
    buf
}

#[async_trait]
impl CacheBackend for InMemoryBackend {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let mut inner = self.inner.lock().await;
        let k = key_bytes(key);

        if let Some((_, entry)) = inner.entries.get(&k).cloned() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            if !entry.invalidator.is_valid(now) {
                inner.entries.remove(&k);
                inner.stats.invalidations += 1;
                inner.stats.misses += 1;
                return Ok(None);
            }

            inner.stats.hits += 1;
            Ok(Some(entry))
        } else {
            inner.stats.misses += 1;
            Ok(None)
        }
    }

    async fn put(&self, key: &CacheKey, entry: CacheEntry) -> Result<(), CacheError> {
        let mut inner = self.inner.lock().await;
        let k = key_bytes(key);
        inner.stats.bytes_stored += entry.size_bytes as u64;
        inner.entries.insert(k, (key.clone(), entry));
        inner.stats.entry_count = inner.entries.len() as u64;
        Ok(())
    }

    async fn delete(&self, key: &CacheKey) -> Result<(), CacheError> {
        let mut inner = self.inner.lock().await;
        let k = key_bytes(key);
        if inner.entries.remove(&k).is_some() {
            inner.stats.entry_count = inner.entries.len() as u64;
        }
        Ok(())
    }

    async fn invalidate_namespace(&self, namespace: &str) -> Result<u64, CacheError> {
        let mut inner = self.inner.lock().await;
        let before = inner.entries.len();
        inner.entries.retain(|_, (k, _)| k.namespace != namespace);
        let removed = (before - inner.entries.len()) as u64;
        inner.stats.invalidations += removed;
        inner.stats.entry_count = inner.entries.len() as u64;
        Ok(removed)
    }

    async fn stats(&self) -> CacheStats {
        let inner = self.inner.lock().await;
        inner.stats.clone()
    }
}
