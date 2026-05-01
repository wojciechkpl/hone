//! Red-phase tests for R12: `schema_version` table-based migrations.
//!
//! These tests pin the public API that the implementation must expose:
//!
//! ```rust
//! const CURRENT_SCHEMA_VERSION: i32 = 2;
//! async fn run_migrations(pool: &SqlitePool) -> Result<i32, MemoryError>;
//! async fn current_schema_version(pool: &SqlitePool) -> Result<i32, MemoryError>;
//! ```
//!
//! All tests FAIL TO COMPILE on main because `current_schema_version` and
//! `CURRENT_SCHEMA_VERSION` do not exist yet.  That is the expected red-phase
//! signal.
//!
//! Design decision pinned by test 2: a db that already has the R1 columns
//! but no `schema_version` table is treated as having skipped v1 and being
//! at v2.  After open, `schema_version` contains exactly one row for v2.
//! Rationale: we cannot distinguish "was v0 ever a separate version in this
//! db" from "was created fresh with R1 schema", so we record only the
//! highest version whose invariants are confirmed.

use hone_memory::{
    sqlite_store::{current_schema_version, CURRENT_SCHEMA_VERSION},
    MemoryKind, MemoryQuery, MemoryStore, SqliteStore,
};

// ---------------------------------------------------------------------------
// Helper: connect a raw sqlx pool to an existing file (no migrations).
// ---------------------------------------------------------------------------
async fn raw_pool(path: &std::path::Path) -> sqlx::SqlitePool {
    let url = format!("sqlite:{}?mode=rwc", path.display());
    sqlx::SqlitePool::connect(&url)
        .await
        .expect("raw_pool: connect")
}

// ---------------------------------------------------------------------------
// Test 1: fresh_db_runs_all_migrations_to_current
// ---------------------------------------------------------------------------

/// Opening a brand-new db file must:
/// - populate `schema_version` with a row for every version up to
///   `CURRENT_SCHEMA_VERSION`, and
/// - return `CURRENT_SCHEMA_VERSION` from `current_schema_version`.
#[tokio::test]
async fn fresh_db_runs_all_migrations_to_current() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    // Closing the NamedTempFile handle while keeping the path is intentional:
    // SqliteStore::open will create/overwrite via mode=rwc.
    drop(tmp);

    let _store = SqliteStore::open(&db_path)
        .await
        .expect("SqliteStore::open on fresh db");

    let pool = raw_pool(&db_path).await;

    let version = current_schema_version(&pool)
        .await
        .expect("current_schema_version after fresh open");

    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "fresh db must be at CURRENT_SCHEMA_VERSION after open"
    );

    // Every version 1..=CURRENT_SCHEMA_VERSION must have a row in schema_version.
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_version")
        .fetch_one(&pool)
        .await
        .expect("count schema_version rows");

    assert_eq!(
        row_count,
        CURRENT_SCHEMA_VERSION as i64,
        "schema_version must have one row per migration (1 through CURRENT_SCHEMA_VERSION)"
    );

    pool.close().await;
}

// ---------------------------------------------------------------------------
// Test 2: legacy_db_with_kind_already_present_records_v2
// ---------------------------------------------------------------------------

/// A db that already has the R1 schema (memories + `kind` + `description`)
/// but NO `schema_version` table: after `SqliteStore::open` the
/// `schema_version` table must exist and contain a row for v2.
///
/// Design decision: we record only v2 (not v1 + v2) because the pre-open db
/// has no `schema_version` table and there is no way to verify whether v1
/// was ever a distinct step.
#[tokio::test]
async fn legacy_db_with_kind_already_present_records_v2() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    drop(tmp);

    // --- Phase A: manually create the R1 schema, no schema_version table ---
    {
        let pool = raw_pool(&db_path).await;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memories (
                id          TEXT    PRIMARY KEY,
                content     TEXT    NOT NULL,
                tags        TEXT    NOT NULL DEFAULT '[]',
                weight      REAL    NOT NULL DEFAULT 1.0,
                created_at  INTEGER NOT NULL,
                decay_at    INTEGER,
                kind        TEXT    NOT NULL DEFAULT 'conversation',
                description TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create R1 memories table");
        pool.close().await;
    }

    // --- Phase B: open with SqliteStore (triggers R12 migration logic) ---
    let _store = SqliteStore::open(&db_path)
        .await
        .expect("SqliteStore::open on R1-schema db");

    let pool = raw_pool(&db_path).await;

    let version = current_schema_version(&pool)
        .await
        .expect("current_schema_version after opening R1 db");

    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "R1 db must reach CURRENT_SCHEMA_VERSION after open"
    );

    // At minimum v2 must be present.
    let v2_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM schema_version WHERE version = 2")
            .fetch_one(&pool)
            .await
            .expect("query v2 row");

    assert_eq!(v2_count, 1, "schema_version must contain exactly one v2 row");

    pool.close().await;
}

// ---------------------------------------------------------------------------
// Test 3: pre_r1_legacy_db_migrates_cleanly
// ---------------------------------------------------------------------------

/// A db with the OLD schema (no `kind`, no `description`):
/// (a) after open `kind` column must exist,
/// (b) the legacy row reads back as `kind = Conversation`, `description = None`,
/// (c) `schema_version` shows v2.
#[tokio::test]
async fn pre_r1_legacy_db_migrates_cleanly() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    drop(tmp);

    // --- Phase A: create the pre-R1 schema and insert a legacy row ---
    {
        let pool = raw_pool(&db_path).await;
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
        .expect("create pre-R1 memories table");

        sqlx::query(
            "INSERT INTO memories (id, content, tags, weight, created_at, decay_at)
             VALUES ('legacy-pre-r1', 'old chat content', '[]', 1.0, 1000000, NULL)",
        )
        .execute(&pool)
        .await
        .expect("insert pre-R1 row");

        pool.close().await;
    }

    // --- Phase B: open via SqliteStore ---
    let store = SqliteStore::open(&db_path)
        .await
        .expect("SqliteStore::open on pre-R1 db");

    // (b) legacy row readable with correct migration defaults.
    let results = store
        .recall(&MemoryQuery {
            text: "old chat".to_string(),
            limit: 10,
        })
        .await
        .expect("recall after migration");

    assert_eq!(results.len(), 1, "legacy row must survive migration");

    let row = &results[0];
    assert_eq!(row.id, "legacy-pre-r1");
    assert_eq!(
        row.kind,
        MemoryKind::Conversation,
        "pre-R1 row must default to MemoryKind::Conversation"
    );
    assert_eq!(
        row.description, None,
        "pre-R1 row description must default to None"
    );

    // (a) + (c): verify via raw pool.
    let pool = raw_pool(&db_path).await;

    // (a) `kind` column must be present.
    let col_rows = sqlx::query("PRAGMA table_info(memories)")
        .fetch_all(&pool)
        .await
        .expect("PRAGMA table_info");
    let col_names: Vec<String> = col_rows
        .iter()
        .filter_map(|r| {
            use sqlx::Row;
            r.try_get::<String, _>("name").ok()
        })
        .collect();
    assert!(
        col_names.contains(&"kind".to_string()),
        "`kind` column must exist after migration; got: {:?}",
        col_names
    );

    // (c) schema_version at v2.
    let version = current_schema_version(&pool)
        .await
        .expect("current_schema_version after pre-R1 migration");

    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "pre-R1 db must reach CURRENT_SCHEMA_VERSION after open"
    );

    pool.close().await;
}

// ---------------------------------------------------------------------------
// Test 4: migrations_are_idempotent
// ---------------------------------------------------------------------------

/// Opening the same db twice must not:
/// - add duplicate `schema_version` rows for already-applied versions, nor
/// - re-run `ALTER TABLE` (which would fail with "duplicate column name"), nor
/// - panic.
///
/// After both opens `schema_version` row count == CURRENT_SCHEMA_VERSION.
#[tokio::test]
async fn migrations_are_idempotent() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    drop(tmp);

    // First open.
    let _store1 = SqliteStore::open(&db_path)
        .await
        .expect("first open");
    drop(_store1);

    // Second open — must not panic or fail.
    let _store2 = SqliteStore::open(&db_path)
        .await
        .expect("second open (idempotent)");
    drop(_store2);

    let pool = raw_pool(&db_path).await;

    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_version")
        .fetch_one(&pool)
        .await
        .expect("count schema_version rows after two opens");

    assert_eq!(
        row_count,
        CURRENT_SCHEMA_VERSION as i64,
        "schema_version must not accumulate duplicate rows across re-opens"
    );

    pool.close().await;
}

// ---------------------------------------------------------------------------
// Test 5: concurrent_migrations_dont_double_apply
// ---------------------------------------------------------------------------

/// Two `SqliteStore::open` calls on the same db file running concurrently
/// must both succeed and leave `schema_version` with exactly
/// `CURRENT_SCHEMA_VERSION` rows.
#[tokio::test]
async fn concurrent_migrations_dont_double_apply() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    drop(tmp);

    let path_a = db_path.clone();
    let path_b = db_path.clone();

    let (result_a, result_b) = tokio::join!(
        SqliteStore::open(&path_a),
        SqliteStore::open(&path_b),
    );

    result_a.expect("concurrent open A must succeed");
    result_b.expect("concurrent open B must succeed");

    let pool = raw_pool(&db_path).await;

    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_version")
        .fetch_one(&pool)
        .await
        .expect("count schema_version rows after concurrent opens");

    assert_eq!(
        row_count,
        CURRENT_SCHEMA_VERSION as i64,
        "concurrent opens must not double-apply migrations; expected {} rows, got {}",
        CURRENT_SCHEMA_VERSION,
        row_count
    );

    pool.close().await;
}

// ---------------------------------------------------------------------------
// Test 6: memory_meta_table_still_works_after_migration
// ---------------------------------------------------------------------------

/// R12 must not regress R11's `memory_meta` table.
///
/// Sequence:
/// 1. Open a fresh db with `open_with_decay_interval(0)` so decay runs
///    immediately and writes `last_decay_at` into `memory_meta`.
/// 2. Re-open the same db.
/// 3. Confirm `last_decay_at` survives and `schema_version` is at
///    `CURRENT_SCHEMA_VERSION`.
#[tokio::test]
async fn memory_meta_table_still_works_after_migration() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let db_path = tmp.path().to_path_buf();
    drop(tmp);

    // Open with interval_secs = 0 so decay always runs and writes memory_meta.
    let _store = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("first open with decay");
    drop(_store);

    // Re-open to confirm memory_meta survives R12 migration on second pass.
    let _store2 = SqliteStore::open(&db_path)
        .await
        .expect("second open after decay");
    drop(_store2);

    let pool = raw_pool(&db_path).await;

    // `last_decay_at` must still be present.
    let last_decay: Option<String> =
        sqlx::query_scalar("SELECT value FROM memory_meta WHERE key = 'last_decay_at'")
            .fetch_optional(&pool)
            .await
            .expect("query memory_meta.last_decay_at");

    assert!(
        last_decay.is_some(),
        "memory_meta.last_decay_at must survive R12 migration; got None"
    );

    // schema_version must also be at CURRENT_SCHEMA_VERSION.
    let version = current_schema_version(&pool)
        .await
        .expect("current_schema_version after R11 regression check");

    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "schema_version must be at CURRENT_SCHEMA_VERSION after re-open"
    );

    pool.close().await;
}
