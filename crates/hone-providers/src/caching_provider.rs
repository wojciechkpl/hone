//! Model-agnostic local response cache.
//!
//! Wraps any `ModelProvider` with an LRU cache keyed on a hash of the
//! request. On cache hit, replays the stored event sequence. On miss,
//! forwards to the inner provider, collects the stream, caches it, and
//! yields events to the caller.
//!
//! Opt-in: enable via `CachingProvider::new(inner, max_entries)`.
//!
//! NOTE: This is an OPT-IN feature intended for:
//! - Development/testing (deterministic replay)
//! - Retry scenarios after transient failures
//! - Idempotent reads where the exact same request may repeat
//!
//! By default, each turn produces a unique request, so cache hits are
//! RARE in production. This is primarily a safety/observability layer.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt, stream};
use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};
use tokio::sync::Mutex;

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

/// Wraps any `ModelProvider` with an in-memory LRU cache.
pub struct CachingProvider {
    inner: Arc<dyn ModelProvider>,
    cache: Arc<Mutex<LruCache>>,
}

impl CachingProvider {
    pub fn new(inner: Arc<dyn ModelProvider>, max_entries: usize) -> Self {
        Self {
            inner,
            cache: Arc::new(Mutex::new(LruCache::new(max_entries))),
        }
    }

    /// Get cache statistics.
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.lock().await;
        CacheStats {
            hits: cache.hits,
            misses: cache.misses,
            entries: cache.entries.len(),
            capacity: cache.capacity,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub capacity: usize,
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

/// Simple LRU cache implementation.
/// Keys are u64 hashes. Values are Vec<ProviderEvent>.
struct LruCache {
    entries: HashMap<u64, Vec<ProviderEvent>>,
    order: VecDeque<u64>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

impl LruCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
            hits: 0,
            misses: 0,
        }
    }

    fn get(&mut self, key: u64) -> Option<Vec<ProviderEvent>> {
        if let Some(events) = self.entries.get(&key).cloned() {
            self.hits += 1;
            // Move key to the back of the LRU queue
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            Some(events)
        } else {
            self.misses += 1;
            None
        }
    }

    fn put(&mut self, key: u64, events: Vec<ProviderEvent>) {
        if self.entries.contains_key(&key) {
            // Refresh position only
            if let Some(pos) = self.order.iter().position(|k| *k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            return;
        }

        // Evict oldest if at capacity
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }

        self.entries.insert(key, events);
        self.order.push_back(key);
    }
}

/// Hash a `CompletionRequest` for cache lookup.
///
/// Includes: model ID, messages (role + text), tools (name only), temperature, max_tokens.
/// Excludes: stop sequences (rarely change behavior in practice).
fn hash_request(req: &CompletionRequest) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    req.model.as_str().hash(&mut hasher);

    for msg in &req.messages {
        format!("{:?}", msg.role).hash(&mut hasher);
        msg.content.as_text().hash(&mut hasher);
    }

    for tool in &req.tools {
        tool.name.hash(&mut hasher);
    }

    req.temperature.map(|t| t.to_bits()).unwrap_or(0).hash(&mut hasher);
    req.max_tokens.unwrap_or(0).hash(&mut hasher);

    hasher.finish()
}

#[async_trait]
impl ModelProvider for CachingProvider {
    fn id(&self) -> &str {
        "caching" // Wrapper ID; inner.id() returns the real provider
    }

    fn display_name(&self) -> &str {
        "Caching Provider (wrapper)"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.inner.supported_models()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let key = hash_request(&request);

        // Check cache
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(key) {
                tracing::debug!(request_hash = key, "cache hit");
                return Ok(Box::pin(stream::iter(cached)));
            }
        }

        tracing::debug!(request_hash = key, "cache miss, forwarding to inner");

        // Miss: forward to inner, collect all events
        let inner_stream = self.inner.complete(request).await?;
        let collected: Vec<ProviderEvent> = inner_stream.collect().await;

        // Store the full sequence
        {
            let mut cache = self.cache.lock().await;
            cache.put(key, collected.clone());
        }

        Ok(Box::pin(stream::iter(collected)))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.inner.estimate_tokens(messages)
    }

    async fn health(&self) -> ProviderHealth {
        self.inner.health().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockProvider;
    use hone_proto::{Message, ModelId};

    #[tokio::test]
    async fn test_cache_miss_forwards_to_inner() {
        let inner = Arc::new(MockProvider::new(vec!["hi".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("query")],
        );

        let stream = cached.complete(req).await.unwrap();
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(!events.is_empty());

        let stats = cached.stats().await;
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.entries, 1);
    }

    #[tokio::test]
    async fn test_cache_hit_returns_cached_events() {
        let inner = Arc::new(MockProvider::new(vec!["cached".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req1 = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("same query")],
        );

        // First call: miss
        let stream = cached.complete(req1.clone()).await.unwrap();
        let _: Vec<_> = stream.collect().await;

        // Second call with identical request: hit
        let stream = cached.complete(req1).await.unwrap();
        let events: Vec<ProviderEvent> = stream.collect().await;
        assert!(!events.is_empty());

        let stats = cached.stats().await;
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hits, 1);
    }

    #[tokio::test]
    async fn test_different_requests_different_cache_keys() {
        let inner = Arc::new(MockProvider::new(vec!["resp".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req1 = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("query A")],
        );
        let req2 = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("query B")],
        );

        let _ = cached.complete(req1).await.unwrap().collect::<Vec<_>>().await;
        let _ = cached.complete(req2).await.unwrap().collect::<Vec<_>>().await;

        let stats = cached.stats().await;
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.entries, 2);
    }

    #[tokio::test]
    async fn test_lru_eviction_at_capacity() {
        let inner = Arc::new(MockProvider::new(vec!["x".to_string()]));
        let cached = CachingProvider::new(inner, 2); // capacity = 2

        for i in 0..3 {
            let req = CompletionRequest::new(
                ModelId::new("mock:test"),
                vec![Message::user(format!("query {i}"))],
            );
            let _ = cached.complete(req).await.unwrap().collect::<Vec<_>>().await;
        }

        let stats = cached.stats().await;
        assert_eq!(stats.entries, 2); // capacity enforced
        assert_eq!(stats.misses, 3);
    }

    #[test]
    fn test_hash_request_deterministic() {
        let req1 = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("hello")],
        );
        let req2 = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("hello")],
        );
        assert_eq!(hash_request(&req1), hash_request(&req2));
    }

    #[test]
    fn test_hash_request_different_models() {
        let req1 = CompletionRequest::new(
            ModelId::new("mock:a"),
            vec![Message::user("hello")],
        );
        let req2 = CompletionRequest::new(
            ModelId::new("mock:b"),
            vec![Message::user("hello")],
        );
        assert_ne!(hash_request(&req1), hash_request(&req2));
    }

    #[tokio::test]
    async fn test_cache_stats_hit_rate() {
        let inner = Arc::new(MockProvider::new(vec!["r".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req = CompletionRequest::new(
            ModelId::new("mock:test"),
            vec![Message::user("same")],
        );

        // 1 miss + 3 hits = 75% hit rate
        for _ in 0..4 {
            let _ = cached.complete(req.clone()).await.unwrap().collect::<Vec<_>>().await;
        }

        let stats = cached.stats().await;
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 0.75).abs() < 1e-9);
    }
}
