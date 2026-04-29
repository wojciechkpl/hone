use hone_proto::SessionId;
use hone_server::SqliteSessionStore;
use hone_server::session::SessionInfo;

fn make_session() -> SessionInfo {
    SessionInfo {
        id: SessionId::new(),
        created_at: 1_700_000_000,
    }
}

#[tokio::test]
async fn test_sqlite_create_and_get() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    let session = make_session();
    let id = session.id.to_string();

    store.create(session.clone()).await.unwrap();

    let fetched = store.get(&id).await.unwrap().expect("session must exist");
    assert_eq!(fetched.id.to_string(), id);
    assert_eq!(fetched.created_at, session.created_at);
}

#[tokio::test]
async fn test_sqlite_persist_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("sessions.db");

    let session = make_session();
    let id = session.id.to_string();

    // First open: create the session.
    {
        let store = SqliteSessionStore::open(&db_path).await.unwrap();
        store.create(session.clone()).await.unwrap();
    }

    // Second open: verify the session survived.
    {
        let store = SqliteSessionStore::open(&db_path).await.unwrap();
        let fetched = store.get(&id).await.unwrap().expect("session must persist");
        assert_eq!(fetched.id.to_string(), id);
        assert_eq!(fetched.created_at, session.created_at);
    }
}

#[tokio::test]
async fn test_sqlite_delete() {
    let store = SqliteSessionStore::in_memory().await.unwrap();
    let session = make_session();
    let id = session.id.to_string();

    store.create(session).await.unwrap();
    assert!(store.get(&id).await.unwrap().is_some());

    let deleted = store.delete(&id).await.unwrap();
    assert!(deleted, "delete should return true for an existing session");

    assert!(store.get(&id).await.unwrap().is_none());

    // Deleting a non-existent session returns false.
    let deleted_again = store.delete(&id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_sqlite_list() {
    let store = SqliteSessionStore::in_memory().await.unwrap();

    for _ in 0..3 {
        store.create(make_session()).await.unwrap();
    }

    let sessions = store.list().await.unwrap();
    assert_eq!(sessions.len(), 3);
}
