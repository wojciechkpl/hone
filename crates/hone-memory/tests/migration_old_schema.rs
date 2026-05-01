//! Red-phase test 4 — opening a SQLite DB created against the old schema
//! (no `kind`, no `description` columns) must succeed and existing rows must
//! read back as `kind = Conversation`, `description = None`.
//!
//! This test will FAIL TO COMPILE on main because:
//!   - `MemoryKind` does not exist yet.
//!   - `Memory` has no `kind` or `description` fields.
//!
//! The runtime behaviour (migration correctness) is the real assertion; the
//! compile failure is acceptable as a red-phase signal.

use hone_memory::{MemoryKind, MemoryQuery, MemoryStore, SqliteStore};

/// Create a SQLite file with the OLD schema (no `kind`, no `description`),
/// insert one row, then reopen it with `SqliteStore::open()` and assert
/// the row reads back with the correct migration defaults.
#[tokio::test]
async fn test_old_schema_migrates_to_default_kind_and_description() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("old_schema.db");

    // --- Phase A: build the OLD schema manually ---
    {
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        let old_pool = sqlx::SqlitePool::connect(&url)
            .await
            .expect("connect to old-schema db");

        // Old CREATE TABLE — no `kind`, no `description`.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id         TEXT    PRIMARY KEY,
                content    TEXT    NOT NULL,
                tags       TEXT    NOT NULL DEFAULT '[]',
                weight     REAL    NOT NULL DEFAULT 1.0,
                created_at INTEGER NOT NULL,
                decay_at   INTEGER
            )",
        )
        .execute(&old_pool)
        .await
        .expect("create old table");

        // Insert one legacy row.
        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, decay_at)
             VALUES ('legacy-1', 'old conversation content', '[]', 1.0, 1000000, NULL)",
        )
        .execute(&old_pool)
        .await
        .expect("insert legacy row");

        old_pool.close().await;
    }

    // --- Phase B: open with SqliteStore (triggers migration) ---
    let store = SqliteStore::open(&db_path)
        .await
        .expect("SqliteStore::open should succeed on old-schema DB");

    // --- Phase C: assert the legacy row has migration defaults ---
    let results = store
        .recall(&MemoryQuery {
            text: "old conversation".to_string(),
            limit: 10,
            kind: None,
        })
        .await
        .expect("recall after migration");

    assert_eq!(
        results.len(),
        1,
        "legacy row should be readable after migration"
    );

    let row = &results[0];
    assert_eq!(row.id, "legacy-1");
    assert_eq!(row.content, "old conversation content");
    assert_eq!(
        row.kind,
        MemoryKind::Conversation,
        "legacy row should default to MemoryKind::Conversation after migration"
    );
    assert_eq!(
        row.description, None,
        "legacy row description should default to None after migration"
    );
}

/// Re-opening the migrated DB a second time must also succeed (idempotent migration).
#[tokio::test]
async fn test_migration_is_idempotent() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("old_schema_idempotent.db");

    // Build old schema.
    {
        let url = format!("sqlite:{}?mode=rwc", db_path.display());
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .expect("connect");
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id         TEXT    PRIMARY KEY,
                content    TEXT    NOT NULL,
                tags       TEXT    NOT NULL DEFAULT '[]',
                weight     REAL    NOT NULL DEFAULT 1.0,
                created_at INTEGER NOT NULL,
                decay_at   INTEGER
            )",
        )
        .execute(&pool)
        .await
        .expect("create old table");
        pool.close().await;
    }

    // First open — runs migration.
    let _ = SqliteStore::open(&db_path)
        .await
        .expect("first open should succeed");

    // Second open — migration must not fail on an already-migrated schema.
    let _ = SqliteStore::open(&db_path)
        .await
        .expect("second open (idempotent) should also succeed");
}
