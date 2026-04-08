use hone_proto::SessionId;
use hone_server::SqliteSessionStore;
use hone_server::session::SessionInfo;

/// Create a session with an explicit `created_at` timestamp.
fn session_at(ts: u64) -> SessionInfo {
    SessionInfo {
        id: SessionId::new(),
        created_at: ts,
    }
}

#[tokio::test]
async fn test_sqlite_gc_removes_old_sessions() {
    let store = SqliteSessionStore::in_memory().await.unwrap();

    // Three sessions: one old, one on the boundary, one recent.
    let old = session_at(1_000);
    let boundary = session_at(5_000);
    let recent = session_at(10_000);

    store.create(old).await.unwrap();
    store.create(boundary.clone()).await.unwrap();
    store.create(recent.clone()).await.unwrap();

    // GC with cutoff=5_000 removes rows where created_at < 5_000 (i.e. only `old`).
    let removed = store.gc(5_000).await.unwrap();
    assert_eq!(removed, 1, "only the old session should be removed");

    let remaining = store.list().await.unwrap();
    assert_eq!(remaining.len(), 2);

    let ids: Vec<String> = remaining.iter().map(|s| s.id.to_string()).collect();
    assert!(ids.contains(&boundary.id.to_string()));
    assert!(ids.contains(&recent.id.to_string()));
}

#[tokio::test]
async fn test_sqlite_gc_removes_all_when_cutoff_high() {
    let store = SqliteSessionStore::in_memory().await.unwrap();

    store.create(session_at(100)).await.unwrap();
    store.create(session_at(200)).await.unwrap();

    // Use a value that fits safely in i64 (SQLite stores timestamps as i64).
    let very_large_cutoff = i64::MAX as u64;
    let removed = store.gc(very_large_cutoff).await.unwrap();
    assert_eq!(removed, 2);
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_sqlite_gc_removes_none_when_cutoff_zero() {
    let store = SqliteSessionStore::in_memory().await.unwrap();

    store.create(session_at(1_000)).await.unwrap();
    store.create(session_at(2_000)).await.unwrap();

    let removed = store.gc(0).await.unwrap();
    assert_eq!(removed, 0);
    assert_eq!(store.list().await.unwrap().len(), 2);
}
