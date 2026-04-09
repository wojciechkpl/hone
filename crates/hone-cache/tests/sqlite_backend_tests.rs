use std::time::{SystemTime, UNIX_EPOCH};

use hone_cache::{
    CacheBackend, CacheEntry, CacheKey, InvalidationTag, SqliteBackend,
};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn make_key(ns: &str, data: &[u8]) -> CacheKey {
    CacheKey::from_bytes(ns, data, InvalidationTag::None)
}

fn make_entry(payload: &[u8]) -> CacheEntry {
    CacheEntry::new(payload.to_vec(), InvalidationTag::None)
}

// ---------------------------------------------------------------------------
// Basic CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_miss_returns_none() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let key = make_key("tool:read_file", b"no_such_entry");
    assert!(backend.get(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn test_put_then_get_returns_entry() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let key = make_key("tool:read_file", b"some_path");
    let entry = make_entry(b"file contents");
    backend.put(&key, entry).await.unwrap();

    let fetched = backend.get(&key).await.unwrap().expect("entry should exist");
    assert_eq!(&*fetched.bytes, b"file contents");
}

#[tokio::test]
async fn test_delete_removes_entry() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let key = make_key("tool:grep", b"pattern");
    backend.put(&key, make_entry(b"match")).await.unwrap();

    backend.delete(&key).await.unwrap();
    assert!(backend.get(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn test_delete_nonexistent_is_ok() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let key = make_key("tool:grep", b"ghost");
    backend.delete(&key).await.unwrap();
}

// ---------------------------------------------------------------------------
// Namespace invalidation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalidate_namespace_removes_all_in_ns() {
    let backend = SqliteBackend::in_memory().await.unwrap();

    let ns = "tool:ls";
    let other_ns = "tool:grep";

    for i in 0u8..3 {
        let key = make_key(ns, &[i]);
        backend.put(&key, make_entry(&[i, i])).await.unwrap();
    }
    let survivor = make_key(other_ns, b"survive");
    backend.put(&survivor, make_entry(b"ok")).await.unwrap();

    let removed = backend.invalidate_namespace(ns).await.unwrap();
    assert_eq!(removed, 3);

    for i in 0u8..3 {
        let key = make_key(ns, &[i]);
        assert!(backend.get(&key).await.unwrap().is_none());
    }
    assert!(backend.get(&survivor).await.unwrap().is_some());
}

#[tokio::test]
async fn test_invalidate_empty_namespace_returns_zero() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let removed = backend.invalidate_namespace("tool:nothing").await.unwrap();
    assert_eq!(removed, 0);
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_stats_tracks_hits_and_misses() {
    let backend = SqliteBackend::in_memory().await.unwrap();
    let key = make_key("tool:read_file", b"data");

    // Two misses.
    backend.get(&key).await.unwrap();
    backend.get(&key).await.unwrap();

    // Store then hit.
    backend.put(&key, make_entry(b"value")).await.unwrap();
    backend.get(&key).await.unwrap();

    let stats = backend.stats().await;
    assert_eq!(stats.misses, 2);
    assert_eq!(stats.hits, 1);
    assert!(stats.hit_rate() > 0.0);
}

// ---------------------------------------------------------------------------
// TTL invalidation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ttl_invalidation_expires() {
    let backend = SqliteBackend::in_memory().await.unwrap();

    let expired_at = now_secs() - 1;
    let tag = InvalidationTag::Ttl(expired_at);
    let key = CacheKey::from_bytes("tool:web", b"url", tag.clone());
    let entry = CacheEntry::new(b"stale data".to_vec(), tag);
    backend.put(&key, entry).await.unwrap();

    let result = backend.get(&key).await.unwrap();
    assert!(result.is_none(), "expired TTL entry should not be returned");

    let stats = backend.stats().await;
    assert_eq!(stats.invalidations, 1);
}

#[tokio::test]
async fn test_ttl_valid_entry_is_returned() {
    let backend = SqliteBackend::in_memory().await.unwrap();

    let far_future = i64::MAX;
    let tag = InvalidationTag::Ttl(far_future);
    let key = CacheKey::from_bytes("tool:web", b"url_v2", tag.clone());
    let entry = CacheEntry::new(b"fresh data".to_vec(), tag);
    backend.put(&key, entry).await.unwrap();

    let result = backend.get(&key).await.unwrap();
    assert!(result.is_some(), "unexpired TTL entry should be returned");
}

// ---------------------------------------------------------------------------
// File hash invalidation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_file_hash_invalidation_on_file_change() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("watched.txt");
    let original = b"original content";
    std::fs::write(&file_path, original).unwrap();

    let correct_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let content = std::fs::read(&file_path).unwrap();
        let mut h = DefaultHasher::new();
        content.hash(&mut h);
        let p = h.finish();
        let mut out = [0u8; 32];
        for i in 0..4usize {
            let seg = p.wrapping_add(i as u64).to_le_bytes();
            out[i * 8..(i + 1) * 8].copy_from_slice(&seg);
        }
        out
    };

    let tag = InvalidationTag::FileHash {
        path: file_path.clone(),
        hash: correct_hash,
    };
    let key = CacheKey::from_bytes("tool:read_file", b"watched.txt", tag.clone());
    let entry = CacheEntry::new(b"cached contents".to_vec(), tag);

    let backend = SqliteBackend::in_memory().await.unwrap();
    backend.put(&key, entry).await.unwrap();

    assert!(
        backend.get(&key).await.unwrap().is_some(),
        "should hit when file is unchanged"
    );

    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .open(&file_path)
        .unwrap();
    f.write_all(b" -- modified").unwrap();
    drop(f);

    assert!(
        backend.get(&key).await.unwrap().is_none(),
        "should miss after file content changed"
    );
}

// ---------------------------------------------------------------------------
// Persistence across reopen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sqlite_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("cache.sqlite");

    let key = make_key("tool:read_file", b"persistent_key");
    let payload = b"persisted value";

    {
        let backend = SqliteBackend::open(&db_path).await.unwrap();
        backend.put(&key, make_entry(payload)).await.unwrap();
        // Backend dropped here, closing the pool.
    }

    {
        let backend = SqliteBackend::open(&db_path).await.unwrap();
        let entry = backend
            .get(&key)
            .await
            .unwrap()
            .expect("entry should survive reopen");
        assert_eq!(&*entry.bytes, payload);
    }
}
