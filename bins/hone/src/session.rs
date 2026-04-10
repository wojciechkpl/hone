//! CLI session persistence backed by SQLite.
//!
//! Sessions are stored in `~/.local/share/hone/sessions.db` with a single
//! table:
//!
//! ```sql
//! CREATE TABLE cli_sessions (
//!     id         TEXT PRIMARY KEY,
//!     name       TEXT UNIQUE NOT NULL,
//!     created_at INTEGER NOT NULL,   -- Unix timestamp (seconds)
//!     updated_at INTEGER NOT NULL,
//!     messages   TEXT NOT NULL        -- JSON-encoded Vec<Message>
//! );
//! ```
//!
//! The `messages` column stores a JSON array so no extra schema migration is
//! needed when `Message` evolves; old rows simply decode with defaults.

use std::path::Path;

use anyhow::{Context, Result};
use hone_proto::Message;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata row returned by `list_sessions`.
#[derive(Debug)]
#[allow(dead_code)] // id and created_at are public API; not all callers use them yet
pub struct SessionMeta {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: usize,
}

// ── Session store ─────────────────────────────────────────────────────────────

/// Handle to the sessions SQLite database.
pub struct SessionStore {
    pool: SqlitePool,
}

impl SessionStore {
    /// Open (or create) the sessions database at `path`.
    pub async fn open(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts)
            .await
            .with_context(|| format!("failed to open sessions db at {}", path.display()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS cli_sessions (
                id         TEXT PRIMARY KEY,
                name       TEXT UNIQUE NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                messages   TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .context("failed to create cli_sessions table")?;

        Ok(Self { pool })
    }

    /// Save (insert or replace) a named session.
    pub async fn save_session(
        &self,
        name: &str,
        messages: &[Message],
    ) -> Result<String> {
        let id = uuid::Uuid::now_v7().to_string();
        let now = unix_now();
        let blob = serde_json::to_string(messages).context("failed to serialize messages")?;

        sqlx::query(
            "INSERT INTO cli_sessions (id, name, created_at, updated_at, messages)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET
                 updated_at = excluded.updated_at,
                 messages   = excluded.messages",
        )
        .bind(&id)
        .bind(name)
        .bind(now)
        .bind(now)
        .bind(&blob)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to save session '{name}'"))?;

        Ok(id)
    }

    /// Load the messages for a named session.  Returns `None` when the session
    /// does not exist.
    pub async fn load_session(&self, name: &str) -> Result<Option<Vec<Message>>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT messages FROM cli_sessions WHERE name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .with_context(|| format!("failed to query session '{name}'"))?;

        match row {
            None => Ok(None),
            Some((blob,)) => {
                let messages: Vec<Message> =
                    serde_json::from_str(&blob).context("failed to deserialize messages")?;
                Ok(Some(messages))
            }
        }
    }

    /// Return metadata for all saved sessions, ordered by `updated_at` descending.
    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let rows: Vec<(String, String, i64, i64, String)> = sqlx::query_as(
            "SELECT id, name, created_at, updated_at, messages
             FROM cli_sessions
             ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to list sessions")?;

        rows.into_iter()
            .map(|(id, name, created_at, updated_at, blob)| {
                let message_count: usize = serde_json::from_str::<Vec<serde_json::Value>>(&blob)
                    .map(|v| v.len())
                    .unwrap_or(0);
                Ok(SessionMeta {
                    id,
                    name,
                    created_at,
                    updated_at,
                    message_count,
                })
            })
            .collect()
    }

    /// Rename a session.  Returns an error when `old_name` does not exist or
    /// `new_name` is already taken.
    pub async fn rename_session(&self, old_name: &str, new_name: &str) -> Result<()> {
        let rows_affected = sqlx::query(
            "UPDATE cli_sessions SET name = ?, updated_at = ? WHERE name = ?",
        )
        .bind(new_name)
        .bind(unix_now())
        .bind(old_name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to rename session '{old_name}' -> '{new_name}'"))?
        .rows_affected();

        anyhow::ensure!(
            rows_affected > 0,
            "session '{old_name}' not found"
        );
        Ok(())
    }

    /// Delete a named session.  Returns an error when the session does not exist.
    pub async fn delete_session(&self, name: &str) -> Result<()> {
        let rows_affected =
            sqlx::query("DELETE FROM cli_sessions WHERE name = ?")
                .bind(name)
                .execute(&self.pool)
                .await
                .with_context(|| format!("failed to delete session '{name}'"))?
                .rows_affected();

        anyhow::ensure!(rows_affected > 0, "session '{name}' not found");
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Format a Unix timestamp as a human-readable local date/time string.
pub fn format_timestamp(ts: i64) -> String {
    // Build a naive UTC datetime from the Unix seconds, then display it in a
    // compact ISO-ish format.  We avoid pulling in a date/time library for this
    // simple display case by doing the arithmetic manually.
    let secs = ts as u64;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;

    // Gregorian calendar: approximate year/month/day from days_since_epoch.
    // Good enough for display purposes (not accounting for local timezone).
    let (year, month, day) = days_to_ymd(days_since_epoch as u32);

    format!("{year}-{month:02}-{day:02} {h:02}:{m:02} UTC")
}

fn days_to_ymd(days: u32) -> (u32, u32, u32) {
    // Algorithm: civil date from days since 1970-01-01 (Howard Hinnant's approach).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m, d)
}

// ── Generate default session name ─────────────────────────────────────────────

/// Generate a default session name like `"session-2026-04-09-1430"` from the
/// current wall-clock time.
pub fn default_session_name() -> String {
    let ts = unix_now() as u64;
    let days = ts / 86400;
    let time_of_day = ts % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let (y, mo, d) = days_to_ymd(days as u32);
    format!("session-{y}-{mo:02}-{d:02}-{h:02}{m:02}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::Message;
    use tempfile::tempdir;

    async fn open_test_store() -> (SessionStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("sessions.db");
        let store = SessionStore::open(&path).await.expect("open");
        (store, dir)
    }

    fn make_messages(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!("user turn {i}"))
                } else {
                    Message::assistant(format!("assistant turn {i}"))
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (store, _dir) = open_test_store().await;
        let msgs = make_messages(4);
        store.save_session("test-session", &msgs).await.unwrap();
        let loaded = store.load_session("test-session").await.unwrap().unwrap();
        assert_eq!(loaded.len(), msgs.len());
        for (orig, loaded) in msgs.iter().zip(loaded.iter()) {
            assert_eq!(orig.role, loaded.role);
        }
    }

    #[tokio::test]
    async fn load_nonexistent_returns_none() {
        let (store, _dir) = open_test_store().await;
        let result = store.load_session("does-not-exist").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_sessions_returns_metadata() {
        let (store, _dir) = open_test_store().await;
        store.save_session("alpha", &make_messages(2)).await.unwrap();
        store.save_session("beta", &make_messages(6)).await.unwrap();

        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);

        // Verify both sessions are present with correct message counts
        // (do not assume ordering when both share the same second-granularity timestamp).
        let alpha = list.iter().find(|s| s.name == "alpha").expect("alpha");
        let beta = list.iter().find(|s| s.name == "beta").expect("beta");
        assert_eq!(alpha.message_count, 2);
        assert_eq!(beta.message_count, 6);
    }

    #[tokio::test]
    async fn save_session_upserts_on_conflict() {
        let (store, _dir) = open_test_store().await;
        store.save_session("dup", &make_messages(2)).await.unwrap();
        store.save_session("dup", &make_messages(8)).await.unwrap();

        let loaded = store.load_session("dup").await.unwrap().unwrap();
        assert_eq!(loaded.len(), 8);

        let list = store.list_sessions().await.unwrap();
        assert_eq!(list.len(), 1, "upsert must not create a second row");
    }

    #[tokio::test]
    async fn rename_session() {
        let (store, _dir) = open_test_store().await;
        store.save_session("old-name", &make_messages(2)).await.unwrap();
        store.rename_session("old-name", "new-name").await.unwrap();

        assert!(store.load_session("old-name").await.unwrap().is_none());
        assert!(store.load_session("new-name").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn rename_nonexistent_errors() {
        let (store, _dir) = open_test_store().await;
        let err = store.rename_session("ghost", "whatever").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn delete_session() {
        let (store, _dir) = open_test_store().await;
        store.save_session("to-delete", &make_messages(2)).await.unwrap();
        store.delete_session("to-delete").await.unwrap();
        assert!(store.load_session("to-delete").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_errors() {
        let (store, _dir) = open_test_store().await;
        let err = store.delete_session("ghost").await;
        assert!(err.is_err());
    }

    #[test]
    fn default_session_name_format() {
        let name = default_session_name();
        // Must look like "session-YYYY-MM-DD-HHMM"
        assert!(name.starts_with("session-"), "got: {name}");
        let parts: Vec<&str> = name.splitn(2, '-').collect();
        assert_eq!(parts[0], "session");
    }

    #[test]
    fn format_timestamp_epoch() {
        let s = format_timestamp(0);
        assert!(s.starts_with("1970-01-01"));
    }
}
