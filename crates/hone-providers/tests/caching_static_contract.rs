//! RED-PHASE regression tests for **MAJOR-8 / MAJOR-9**: `CachingProvider`
//! uses `std::collections::hash_map::DefaultHasher` (not stable across
//! rustc versions; not appropriate for a persistable cache key) and
//! deep-clones the entire event vector on every cache miss.
//!
//! Audit findings (g.1) and (g.2):
//!     `crates/hone-providers/src/caching_provider.rs:158`  — DefaultHasher
//!     `crates/hone-providers/src/caching_provider.rs:218`  — `collected.clone()`
//!
//! Why static-source tests:
//!     `hash_request` is a private free function and `LruCache` stores
//!     `Vec<ProviderEvent>` by value — neither is reachable from an
//!     integration test (a separate crate).  We could test the observable
//!     behaviour through `complete()` cache stats, but those already pass
//!     under `DefaultHasher` (the bug is cross-rustc-version, not within a
//!     single test run).  The cleanest way to enforce the coding rule
//!     "use a stable hasher; don't deep-clone the cache value" is a
//!     substring assertion on the source.
//!
//! These tests will FAIL against current `main` because:
//!     - Line 158 contains the literal `DefaultHasher`.
//!     - Line 218 contains the literal `collected.clone()` immediately
//!       before passing into `cache.put`.
//!
//! Implementer's freedom (green phase):
//!     - Pick any stable hasher: `siphasher::SipHasher13` (deterministic
//!       seed of zero), `xxhash_rust::xxh3::Xxh3`, or
//!       `blake3::hash(...).as_bytes()[..8]`.  The chosen crate must be
//!       added to `Cargo.toml`.
//!     - For the cache value, switch to `Arc<Vec<ProviderEvent>>` so the
//!       returned stream and the cache entry share the underlying
//!       allocation.  Update `LruCache` storage and the `get`/`put`
//!       signatures accordingly.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use futures::StreamExt;
use hone_proto::{Message, ModelId};
// `ModelProvider` is brought into scope so `cached.complete(req)` resolves.
use hone_providers::traits::{CompletionRequest, ModelProvider};
use hone_providers::{CachingProvider, MockProvider};

const CACHING_PROVIDER_SRC: &str = include_str!("../src/caching_provider.rs");

// ---------------------------------------------------------------------------
// Finding (g.1): hash_request must use a deterministic, version-stable hasher.
// ---------------------------------------------------------------------------

#[test]
fn hash_request_does_not_use_default_hasher() {
    assert!(
        !CACHING_PROVIDER_SRC.contains("DefaultHasher"),
        "AUDIT-MAJOR-9: caching_provider.rs must not use \
         std::collections::hash_map::DefaultHasher.  The stdlib explicitly \
         documents that DefaultHasher's output is not stable across Rust \
         releases, so a persistent or shared cache becomes invisible after a \
         rebuild.  Replace with siphasher, xxhash_rust, or blake3 (truncated \
         to u64)."
    );
}

#[test]
fn caching_provider_imports_a_stable_hasher_crate() {
    // Positive contract: the source must reference at least one of the
    // common deterministic hashers.  This guards against deleting
    // DefaultHasher and accidentally falling through to no hashing at all
    // (e.g. `let key = 0u64;` would silently make every request collide).
    let approved = ["siphasher", "xxhash", "blake3", "ahash::AHasher", "fnv::"];

    let used = approved
        .iter()
        .any(|needle| CACHING_PROVIDER_SRC.contains(needle));

    assert!(
        used,
        "AUDIT-MAJOR-9: caching_provider.rs must reference a deterministic \
         hasher.  Looked for any of: {approved:?}.  Found none — has the \
         hash been replaced with a constant or removed entirely?"
    );
}

// ---------------------------------------------------------------------------
// Finding (g.2): the cache miss path must not deep-clone the event vector.
// ---------------------------------------------------------------------------

#[test]
fn complete_does_not_deep_clone_collected_events() {
    // The forbidden pattern is the literal `collected.clone()` that today
    // passes a deep copy into `cache.put` and then reconstructs the stream
    // from the original Vec.  After the fix, the cache stores
    // `Arc<Vec<ProviderEvent>>` and the returned stream borrows the same
    // Arc — no deep clone.
    assert!(
        !CACHING_PROVIDER_SRC.contains("collected.clone()"),
        "AUDIT-MAJOR-8: caching_provider.rs::complete must not deep-clone \
         the event vector on every cache miss.  Switch the cache value to \
         Arc<Vec<ProviderEvent>> and yield via `stream::iter` over an \
         Arc-cloned slice."
    );
}

#[test]
fn lru_cache_value_type_is_shared() {
    // Positive contract: the cache must store events behind an Arc so the
    // returned stream and the cache entry share the same allocation.
    // We accept either form: `Arc<Vec<ProviderEvent>>` or
    // `Arc<[ProviderEvent]>`.
    let stores_via_arc = CACHING_PROVIDER_SRC.contains("Arc<Vec<ProviderEvent>>")
        || CACHING_PROVIDER_SRC.contains("Arc<[ProviderEvent]>");

    assert!(
        stores_via_arc,
        "AUDIT-MAJOR-8: LruCache must store cached events as \
         Arc<Vec<ProviderEvent>> (or Arc<[ProviderEvent]>) so cache hits \
         and the post-miss stream share the same allocation."
    );
}

// ---------------------------------------------------------------------------
// Finding (g): observable behavioural regression — same input, same hash.
//
// This test ALREADY passes on current main (DefaultHasher is deterministic
// within a single process), but pins the contract through the public
// surface so the green-phase fix doesn't accidentally regress it (e.g. by
// seeding a fresh RandomState on every call).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cache_hit_after_identical_request_remains_observable() {
    let inner = Arc::new(MockProvider::new(vec!["hi".to_string()]));
    let cached = CachingProvider::new(inner, 10);

    let req = CompletionRequest::new(
        ModelId::new("mock:test"),
        vec![Message::user("identical-prompt-for-stability-check")],
    );

    // First call: miss.
    let _ = cached
        .complete(req.clone())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;

    // Second call with the *same* logical request: must hit.
    let _ = cached
        .complete(req)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;

    let stats = cached.stats().await;
    assert_eq!(
        stats.hits, 1,
        "regression: the chosen hasher must produce the same key for \
         identical requests within a single process.  Got hits={}, misses={}",
        stats.hits, stats.misses
    );
    assert_eq!(stats.misses, 1);
}
