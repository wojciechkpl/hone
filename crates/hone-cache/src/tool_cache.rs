use std::future::Future;
use std::sync::Arc;

use crate::backend::{CacheBackend, CacheEntry, CacheError};
use crate::key::{CacheKey, InvalidationTag};

/// High-level API for caching tool results.
///
/// Tools use this via [`get_or_compute`]: the cache either returns a stored
/// entry or computes one by invoking the supplied closure and stores the
/// result before returning it to the caller.
///
/// [`get_or_compute`]: ToolResultCache::get_or_compute
pub struct ToolResultCache {
    backend: Arc<dyn CacheBackend>,
}

impl ToolResultCache {
    pub fn new(backend: Arc<dyn CacheBackend>) -> Self {
        Self { backend }
    }

    /// Look up a cached entry, or compute and store one on miss.
    ///
    /// Returns `(bytes, cache_hit)`.  `cache_hit` is `true` iff the entry was
    /// served from the cache without invoking `compute`.  The flag is set
    /// atomically with the read, eliminating the TOCTOU race that a separate
    /// probe would introduce.
    ///
    /// The `compute` closure returns `(bytes, invalidator)`.  The invalidator
    /// is attached to the stored entry so that future lookups can determine
    /// whether the cached value is still valid.
    pub async fn get_or_compute<F, Fut>(
        &self,
        key: CacheKey,
        compute: F,
    ) -> Result<(Vec<u8>, bool), CacheError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(Vec<u8>, InvalidationTag), CacheError>>,
    {
        if let Some(entry) = self.backend.get(&key).await? {
            tracing::debug!(namespace = %key.namespace, "tool cache hit");
            return Ok((entry.bytes.to_vec(), true));
        }

        tracing::debug!(namespace = %key.namespace, "tool cache miss — computing");
        let (bytes, invalidator) = compute().await?;

        // Update the key's invalidator to the one returned by the compute
        // closure (callers may not know the invalidator until after computing).
        let mut stored_key = key;
        stored_key.invalidator = invalidator.clone();
        let entry = CacheEntry::new(bytes.clone(), invalidator);
        self.backend.put(&stored_key, entry).await?;

        Ok((bytes, false))
    }

    /// Access the underlying backend (e.g. to call `stats` or
    /// `invalidate_namespace` directly).
    pub fn backend(&self) -> &Arc<dyn CacheBackend> {
        &self.backend
    }
}
