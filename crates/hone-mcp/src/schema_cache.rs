use std::collections::HashMap;
use std::future::Future;

use tokio::sync::RwLock;

use crate::error::McpError;
use crate::traits::McpToolDef;

/// Lazy, per-server schema cache.
///
/// Schemas are loaded on first access and then served from memory for all
/// subsequent calls.  The cache never evicts entries — server schemas are
/// assumed to be stable for the lifetime of the process.
pub struct SchemaCache {
    inner: RwLock<HashMap<String, Vec<McpToolDef>>>,
}

impl SchemaCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Return cached schemas for `server_name`, or invoke `loader` once to
    /// populate the cache.
    ///
    /// The `loader` closure is called **at most once** per server name across
    /// the lifetime of this cache instance, even under concurrent access.
    pub async fn get_or_load<F, Fut>(
        &self,
        server_name: &str,
        loader: F,
    ) -> Result<Vec<McpToolDef>, McpError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<McpToolDef>, McpError>>,
    {
        // Fast path: read lock — entry already cached.
        {
            let map = self.inner.read().await;
            if let Some(defs) = map.get(server_name) {
                return Ok(defs.clone());
            }
        }

        // Slow path: write lock — load and insert.
        let mut map = self.inner.write().await;

        // Re-check after acquiring write lock (another task may have loaded
        // while we were waiting).
        if let Some(defs) = map.get(server_name) {
            return Ok(defs.clone());
        }

        let defs = loader().await?;
        map.insert(server_name.to_string(), defs.clone());
        Ok(defs)
    }
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}
