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
/// Keys are u64 hashes. Values are Arc<Vec<ProviderEvent>> so callers share
/// the allocation without deep-cloning.
struct LruCache {
    entries: HashMap<u64, Arc<Vec<ProviderEvent>>>,
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

    fn get(&mut self, key: u64) -> Option<Arc<Vec<ProviderEvent>>> {
        if let Some(events) = self.entries.get(&key).map(Arc::clone) {
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

    // `clippy::map_entry` would prefer `Entry::or_insert_with` here, but the
    // Entry API doesn't compose with the `order: VecDeque` we maintain
    // alongside `entries`: the existing-key branch needs to mutate the deque
    // (refresh position) and the new-key branch needs to evict-then-insert,
    // which means borrowing `self.entries` again while an Entry handle is
    // alive.  A `contains_key` lookup is the cleanest split here.
    #[allow(clippy::map_entry)]
    fn put(&mut self, key: u64, events: Arc<Vec<ProviderEvent>>) {
        // Existing key: overwrite the value AND refresh LRU position.
        //
        // The previous implementation only refreshed position and silently
        // dropped `events`.  Today `CachingProvider::complete` never calls
        // `put` for an already-cached key (it short-circuits on hit), so the
        // bug is latent — but a future refactor that adds cache warming, a
        // refresh policy, or concurrent put-on-miss would silently serve the
        // stale value forever.  Fix it at the source.
        if self.entries.contains_key(&key) {
            self.entries.insert(key, events);
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
/// Uses `blake3` for a deterministic, version-stable hash that will not
/// change across Rust releases (unlike the stdlib default hasher).
///
/// Includes: model ID, messages (role + text), tools (name only), temperature, max_tokens.
/// Excludes: stop sequences (rarely change behavior in practice); `cancel`
/// (a `CancellationToken` is not `Hash` and is not semantically a cache key —
/// two requests with identical content but different cancel handles must hit
/// the same cache entry).
fn hash_request(req: &CompletionRequest) -> u64 {
    let mut hasher = blake3::Hasher::new();

    hasher.update(req.model.as_str().as_bytes());

    for msg in &req.messages {
        hasher.update(format!("{:?}", msg.role).as_bytes());
        hasher.update(msg.content.as_text().as_bytes());
    }

    for tool in &req.tools {
        hasher.update(tool.name.as_bytes());
    }

    let temp_bits = req.temperature.map(|t| t.to_bits()).unwrap_or(0);
    hasher.update(&temp_bits.to_le_bytes());

    let max_tokens = req.max_tokens.unwrap_or(0) as u64;
    hasher.update(&max_tokens.to_le_bytes());

    let hash_bytes = hasher.finalize();
    // blake3 output is always 32 bytes; take the first 8 as a stable u64 key.
    let b = hash_bytes.as_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
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
                // The Vec lives in the LRU once; here we element-clone it out
                // of the shared Arc to feed `stream::iter`. ProviderEvent is
                // Clone but not Copy, so each event is duplicated. Acceptable
                // because cache hits are rare and the Vec itself is not.
                let events: Vec<ProviderEvent> = (*cached).clone();
                return Ok(Box::pin(stream::iter(events)));
            }
        }

        tracing::debug!(request_hash = key, "cache miss, forwarding to inner");

        // Miss: forward to inner, collect all events, wrap in Arc for shared ownership.
        let inner_stream = self.inner.complete(request).await?;
        let collected: Arc<Vec<ProviderEvent>> = Arc::new(inner_stream.collect().await);

        // Store the full sequence
        {
            let mut cache = self.cache.lock().await;
            cache.put(key, Arc::clone(&collected));
        }

        // The Vec is now owned by the LRU; element-clone it out of the
        // shared Arc to feed `stream::iter`. See cache-hit branch above
        // for why the per-event clone is acceptable.
        let events: Vec<ProviderEvent> = (*collected).clone();
        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.inner.estimate_tokens(messages)
    }

    async fn health(&self) -> ProviderHealth {
        self.inner.health().await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::MockProvider;
    use hone_proto::{Message, ModelId};

    #[tokio::test]
    async fn test_cache_miss_forwards_to_inner() {
        let inner = Arc::new(MockProvider::new(vec!["hi".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req = CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("query")]);

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

        let req1 =
            CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("same query")]);

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

        let req1 =
            CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("query A")]);
        let req2 =
            CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("query B")]);

        let _ = cached
            .complete(req1)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        let _ = cached
            .complete(req2)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;

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
            let _ = cached
                .complete(req)
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;
        }

        let stats = cached.stats().await;
        assert_eq!(stats.entries, 2); // capacity enforced
        assert_eq!(stats.misses, 3);
    }

    #[test]
    fn test_hash_request_deterministic() {
        let req1 = CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("hello")]);
        let req2 = CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("hello")]);
        assert_eq!(hash_request(&req1), hash_request(&req2));
    }

    #[test]
    fn test_hash_request_different_models() {
        let req1 = CompletionRequest::new(ModelId::new("mock:a"), vec![Message::user("hello")]);
        let req2 = CompletionRequest::new(ModelId::new("mock:b"), vec![Message::user("hello")]);
        assert_ne!(hash_request(&req1), hash_request(&req2));
    }

    // -----------------------------------------------------------------------
    // LruCache::put correctness — regression test for a latent bug.
    //
    // The previous implementation, on the existing-key branch, refreshed the
    // LRU position and returned without overwriting the value.  This is not
    // reachable through `CachingProvider::complete` today (cache hits short-
    // circuit before put is called) but the function-internal contract was
    // wrong and any future cache-warming/refresh path would silently serve
    // stale events.  These tests pin the corrected behaviour.
    // -----------------------------------------------------------------------

    #[test]
    fn test_lru_put_overwrites_existing_value() {
        let mut cache = LruCache::new(10);
        let key = 42u64;

        let v1 = Arc::new(vec![ProviderEvent::Token {
            text: "first".into(),
        }]);
        let v2 = Arc::new(vec![ProviderEvent::Token {
            text: "second".into(),
        }]);

        cache.put(key, Arc::clone(&v1));
        cache.put(key, Arc::clone(&v2));

        let got = cache.get(key).expect("entry must be cached");
        assert_eq!(
            got, v2,
            "put on an existing key must overwrite the value, not silently drop the new payload"
        );
    }

    #[test]
    fn test_lru_put_existing_key_does_not_double_count_entries() {
        // Re-putting the same key must not grow the entries table — the
        // value-overwrite path must update in place, not insert-then-leak.
        let mut cache = LruCache::new(10);
        let key = 7u64;

        cache.put(
            key,
            Arc::new(vec![ProviderEvent::Token { text: "a".into() }]),
        );
        cache.put(
            key,
            Arc::new(vec![ProviderEvent::Token { text: "b".into() }]),
        );
        cache.put(
            key,
            Arc::new(vec![ProviderEvent::Token { text: "c".into() }]),
        );

        assert_eq!(
            cache.entries.len(),
            1,
            "three puts on the same key must produce exactly one entry"
        );
        assert_eq!(
            cache.order.len(),
            1,
            "order deque must also contain exactly one position for the key"
        );
    }

    #[test]
    fn test_lru_put_existing_key_refreshes_position() {
        // Putting an existing key must move it to the back of the LRU queue,
        // protecting it from eviction even when newer keys arrive.
        let mut cache = LruCache::new(2);
        cache.put(
            1,
            Arc::new(vec![ProviderEvent::Token { text: "one".into() }]),
        );
        cache.put(
            2,
            Arc::new(vec![ProviderEvent::Token { text: "two".into() }]),
        );

        // Re-put key 1 with an updated value — should move it to back AND
        // overwrite the value.
        cache.put(
            1,
            Arc::new(vec![ProviderEvent::Token {
                text: "one-v2".into(),
            }]),
        );

        // Insert a third key — this evicts the oldest, which should now be
        // key 2 (because key 1 was just refreshed).
        cache.put(
            3,
            Arc::new(vec![ProviderEvent::Token {
                text: "three".into(),
            }]),
        );

        assert!(
            cache.entries.contains_key(&1),
            "refreshed key 1 must survive eviction"
        );
        assert!(
            !cache.entries.contains_key(&2),
            "key 2 must be evicted as the oldest"
        );
        assert!(
            cache.entries.contains_key(&3),
            "newest key 3 must be present"
        );

        // And key 1's value reflects the most recent put.
        let v1 = cache.get(1).expect("key 1 cached");
        assert_eq!(
            *v1,
            vec![ProviderEvent::Token {
                text: "one-v2".into(),
            }],
        );
    }

    #[tokio::test]
    async fn test_cache_stats_hit_rate() {
        let inner = Arc::new(MockProvider::new(vec!["r".to_string()]));
        let cached = CachingProvider::new(inner, 10);

        let req = CompletionRequest::new(ModelId::new("mock:test"), vec![Message::user("same")]);

        // 1 miss + 3 hits = 75% hit rate
        for _ in 0..4 {
            let _ = cached
                .complete(req.clone())
                .await
                .unwrap()
                .collect::<Vec<_>>()
                .await;
        }

        let stats = cached.stats().await;
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate() - 0.75).abs() < 1e-9);
    }
}
