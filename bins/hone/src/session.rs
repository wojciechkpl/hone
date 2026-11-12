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
use hone_proto::{ContextReceipt, Message};
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

/// A context receipt as stored with a session. `run` numbers the processes that
/// saved to the session (turn numbers restart when a session is resumed), and
/// `seq` orders the receipts within a run.
#[derive(Debug, Clone)]
pub struct StoredReceipt {
    pub run: u32,
    pub seq: u32,
    pub receipt: ContextReceipt,
}

// ── Session store ─────────────────────────────────────────────────────────────

/// Handle to the sessions SQLite database.
pub struct SessionStore {
    pool: SqlitePool,
    /// Per (session id, agent id): the run this store instance writes receipts
    /// to, and the last ordinal it has stored there.
    receipt_runs: tokio::sync::Mutex<std::collections::HashMap<(String, String), (u32, u64)>>,
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

        // Context receipts, keyed by the session id (a rename keeps them; a
        // delete removes them). Created in place for databases made before.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS context_receipts (
                session_id TEXT NOT NULL REFERENCES cli_sessions(id) ON DELETE CASCADE,
                run        INTEGER NOT NULL,
                seq        INTEGER NOT NULL,
                turn       INTEGER NOT NULL,
                call       INTEGER NOT NULL,
                receipt    TEXT NOT NULL,
                PRIMARY KEY (session_id, run, seq)
            )",
        )
        .execute(&pool)
        .await
        .context("failed to create context_receipts table")?;

        Ok(Self {
            pool,
            receipt_runs: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Store the agent's context receipts (`AgentHandle::context_receipts_after`)
    /// with the session. A run is one (store instance, session, agent id): its
    /// first batch starts a new run (`max(run) + 1`), `seq` is the receipts'
    /// ordinal, and only ordinals above what that run already holds are stored,
    /// so repeated or overlapping batches don't duplicate. A gap (the agent's cap
    /// dropped old receipts) is logged, and later receipts are still stored.
    /// The run number and the rows are written in one `BEGIN IMMEDIATE`
    /// transaction, so concurrent first saves from two stores get distinct runs.
    /// Errors when the session does not exist.
    pub async fn append_context_receipts(
        &self,
        session_name: &str,
        batch: &hone_core::ReceiptBatch,
    ) -> Result<()> {
        let id = self
            .session_id(session_name)
            .await?
            .with_context(|| format!("session '{session_name}' not found"))?;

        let mut runs = self.receipt_runs.lock().await;
        let key = (id.clone(), batch.agent_id.clone());
        let known = runs.get(&key).copied();
        let stored = known.map_or(0, |(_, last)| last);
        let fresh: Vec<&(u64, ContextReceipt)> =
            batch.items.iter().filter(|(o, _)| *o > stored).collect();
        let Some(first) = fresh.first().map(|(o, _)| *o) else {
            return Ok(());
        };
        if first > stored + 1 {
            tracing::warn!(
                session = session_name,
                from = stored + 1,
                to = first - 1,
                "context receipts were dropped by the agent's cap before they were stored"
            );
        }

        let mut conn = self
            .pool
            .acquire()
            .await
            .context("failed to get a database connection")?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .context("failed to start the receipt transaction")?;
        let result: Result<u32> = async {
            let run = match known {
                Some((run, _)) => run,
                None => {
                    let (max_run,): (i64,) = sqlx::query_as(
                        "SELECT COALESCE(MAX(run), 0) FROM context_receipts WHERE session_id = ?",
                    )
                    .bind(&id)
                    .fetch_one(&mut *conn)
                    .await
                    .context("failed to read the receipt runs")?;
                    max_run as u32 + 1
                }
            };
            for (ordinal, receipt) in &fresh {
                let json =
                    serde_json::to_string(receipt).context("failed to serialize a receipt")?;
                sqlx::query(
                    "INSERT INTO context_receipts (session_id, run, seq, turn, call, receipt)
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(run as i64)
                .bind(*ordinal as i64)
                .bind(receipt.turn as i64)
                .bind(receipt.call as i64)
                .bind(json)
                .execute(&mut *conn)
                .await
                .context("failed to store a context receipt")?;
            }
            Ok(run)
        }
        .await;
        match result {
            Ok(run) => {
                sqlx::query("COMMIT")
                    .execute(&mut *conn)
                    .await
                    .context("failed to commit the receipts")?;
                let last = fresh.last().map_or(stored, |(o, _)| *o);
                runs.insert(key, (run, last));
                Ok(())
            }
            Err(e) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(e)
            }
        }
    }

    /// Open the sessions database read-only: no creation, no schema changes.
    /// For `hone context show`.
    pub async fn open_read_only(path: &Path) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePool::connect_with(opts)
            .await
            .with_context(|| format!("failed to open sessions db at {}", path.display()))?;
        Ok(Self {
            pool,
            receipt_runs: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Resolve a session by name or id without loading its messages; `None`
    /// resolves to the most recently updated session.
    pub async fn resolve_session_name(&self, key: Option<&str>) -> Result<Option<String>> {
        match key {
            None => self.latest_session_name().await,
            Some(k) => {
                let row: Option<(String,)> = sqlx::query_as(
                    "SELECT name FROM cli_sessions WHERE name = ?1 OR id = ?1
                     ORDER BY (name = ?1) DESC LIMIT 1",
                )
                .bind(k)
                .fetch_optional(&self.pool)
                .await
                .with_context(|| format!("failed to look up session '{k}'"))?;
                Ok(row.map(|(n,)| n))
            }
        }
    }

    /// Every stored receipt of the session, ordered by `(run, seq)`. Empty for an
    /// unknown session or one without receipts.
    pub async fn list_context_receipts(&self, session_name: &str) -> Result<Vec<StoredReceipt>> {
        let (tables,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'context_receipts'",
        )
        .fetch_one(&self.pool)
        .await
        .context("failed to inspect the sessions db")?;
        if tables == 0 {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT r.run, r.seq, r.receipt
             FROM context_receipts r JOIN cli_sessions s ON s.id = r.session_id
             WHERE s.name = ?
             ORDER BY r.run, r.seq",
        )
        .bind(session_name)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("failed to read the receipts of session '{session_name}'"))?;
        rows.into_iter()
            .map(|(run, seq, json)| {
                Ok(StoredReceipt {
                    run: run as u32,
                    seq: seq as u32,
                    receipt: serde_json::from_str(&json).context("bad stored receipt")?,
                })
            })
            .collect()
    }

    /// The id of the session called `name`, if there is one.
    async fn session_id(&self, name: &str) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT id FROM cli_sessions WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .with_context(|| format!("failed to look up session '{name}'"))?;
        Ok(row.map(|(id,)| id))
    }

    /// The name of the most recently updated session, if any.
    pub async fn latest_session_name(&self) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT name FROM cli_sessions ORDER BY updated_at DESC, rowid DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .context("failed to find the latest session")?;
        Ok(row.map(|(n,)| n))
    }

    /// Save (insert or replace) a named session.
    pub async fn save_session(&self, name: &str, messages: &[Message]) -> Result<String> {
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
        let rows_affected =
            sqlx::query("UPDATE cli_sessions SET name = ?, updated_at = ? WHERE name = ?")
                .bind(new_name)
                .bind(unix_now())
                .bind(old_name)
                .execute(&self.pool)
                .await
                .with_context(|| format!("failed to rename session '{old_name}' -> '{new_name}'"))?
                .rows_affected();

        anyhow::ensure!(rows_affected > 0, "session '{old_name}' not found");
        Ok(())
    }

    /// Delete a named session.  Returns an error when the session does not exist.
    pub async fn delete_session(&self, name: &str) -> Result<()> {
        // Receipts first (the foreign key would cascade, but only when enabled).
        sqlx::query(
            "DELETE FROM context_receipts
             WHERE session_id IN (SELECT id FROM cli_sessions WHERE name = ?)",
        )
        .bind(name)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to delete the receipts of session '{name}'"))?;
        let rows_affected = sqlx::query("DELETE FROM cli_sessions WHERE name = ?")
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

/// Generate a default session name in funky format, e.g. `"groovy-otter-1430-7f"`.
pub fn default_session_name() -> String {
    hone_core::agent_name::name_for_agent_type(None)
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
        store
            .save_session("alpha", &make_messages(2))
            .await
            .unwrap();
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
        store
            .save_session("old-name", &make_messages(2))
            .await
            .unwrap();
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
        store
            .save_session("to-delete", &make_messages(2))
            .await
            .unwrap();
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
    fn default_session_name_funky_format() {
        let name = default_session_name();
        let re = regex::Regex::new(r"^[a-z]+-[a-z]+-\d{4}-[0-9a-f]{2}$").unwrap();
        assert!(
            re.is_match(&name),
            "default_session_name() returned {name:?}; expected funky format ^[a-z]+-[a-z]+-\\d{{4}}-[0-9a-f]{{2}}$"
        );
    }

    // ── Test 11 ───────────────────────────────────────────────────────────────
    // Likely fails TODAY: current format truncates to minute resolution, so two
    // consecutive calls in the same minute return the same string.
    // Passes AFTER green phase adds the nanosecond-seeded XX suffix.
    #[test]
    fn default_session_name_changes_per_call() {
        let first = default_session_name();
        std::hint::black_box(&first);
        let second = default_session_name();
        assert_ne!(
            first, second,
            "consecutive calls to default_session_name() must return different values; \
             the XX collision suffix should disambiguate them"
        );
    }

    #[test]
    fn format_timestamp_epoch() {
        let s = format_timestamp(0);
        assert!(s.starts_with("1970-01-01"));
    }

    // ---- CR11 of docs/plans/context-receipts-spec.md ------------------------------------
    //
    // API required (in this file):
    //
    //   pub struct StoredReceipt { pub run: u32, pub seq: u32, pub receipt: hone_proto::ContextReceipt }
    //       // Debug + Clone
    //   impl SessionStore {
    //       /// Receipts of the agent's life so far (`AgentHandle::get_context_receipts()`),
    //       /// oldest first. The first call of this store instance for a session takes
    //       /// `run = max(run) + 1` (1 for a new one) and stores them all, `seq` 1-based and
    //       /// increasing; later calls for the same session in this instance store only the
    //       /// receipts beyond those already stored. Errors when the session does not exist.
    //       pub async fn append_context_receipts(&self, session_name: &str,
    //           receipts: &[ContextReceipt]) -> Result<()>;
    //       /// Every stored receipt of the session ordered by (run, seq); empty for an
    //       /// unknown session or one without receipts.
    //       pub async fn list_context_receipts(&self, session_name: &str)
    //           -> Result<Vec<StoredReceipt>>;
    //   }
    //
    // The table `context_receipts` is keyed by the session id (so renaming a session keeps
    // its receipts and deleting one deletes them), created in place for old databases.

    fn cr11_receipt(turn: u32, call: u32) -> hone_proto::ContextReceipt {
        hone_proto::ContextReceipt {
            turn,
            call,
            model: "mock:test".to_owned(),
            request_sha256: format!("{turn:02x}{call:02x}").repeat(16),
            blocks: vec![],
            dropped: vec![],
            compaction: vec![],
            tools_offered: vec!["read_file".to_owned()],
            history_len: 0,
            history_root: String::new(),
        }
    }

    /// A batch as `AgentHandle::context_receipts_after` returns it: `receipts` get
    /// the ordinals `first`, `first + 1`, ... (ordinals are 1-based for an agent's life).
    fn batch(
        agent_id: &str,
        first: u64,
        receipts: &[hone_proto::ContextReceipt],
    ) -> hone_core::ReceiptBatch {
        hone_core::ReceiptBatch {
            agent_id: agent_id.to_owned(),
            items: receipts
                .iter()
                .enumerate()
                .map(|(i, r)| (first + i as u64, r.clone()))
                .collect(),
        }
    }

    fn pairs(v: &[StoredReceipt]) -> Vec<(u32, u32, u32, u32)> {
        v.iter()
            .map(|s| (s.run, s.seq, s.receipt.turn, s.receipt.call))
            .collect()
    }

    #[tokio::test]
    async fn cr11_first_append_should_store_every_receipt_as_run_one() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();

        store
            .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1), cr11_receipt(1, 2)]))
            .await
            .unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(pairs(&got), [(1, 1, 1, 1), (1, 2, 1, 2)]);
        assert_eq!(
            got[0].receipt,
            cr11_receipt(1, 1),
            "the receipt round-trips"
        );
    }

    #[tokio::test]
    async fn cr11_later_appends_in_the_same_process_should_store_only_new_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        let mut all = vec![cr11_receipt(1, 1), cr11_receipt(1, 2)];
        store.append_context_receipts("s", &batch("a1", 1, &all)).await.unwrap();

        all.push(cr11_receipt(2, 1));
        store.append_context_receipts("s", &batch("a1", 1, &all)).await.unwrap();
        store.append_context_receipts("s", &batch("a1", 1, &all)).await.unwrap(); // nothing new

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(pairs(&got), [(1, 1, 1, 1), (1, 2, 1, 2), (1, 3, 2, 1)]);
    }

    #[tokio::test]
    async fn cr11_a_new_process_should_take_the_next_run_and_restart_seq() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        {
            let store = SessionStore::open(&path).await.unwrap();
            store.save_session("s", &make_messages(2)).await.unwrap();
            store
                .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1), cr11_receipt(1, 2)]))
                .await
                .unwrap();
        }
        // A second process resumes the session: turn numbers restart.
        let store = SessionStore::open(&path).await.unwrap();
        store
            .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(pairs(&got), [(1, 1, 1, 1), (1, 2, 1, 2), (2, 1, 1, 1)]);
    }

    #[tokio::test]
    async fn cr11_sessions_should_number_their_runs_independently() {
        let (store, _d) = open_test_store().await;
        store.save_session("a", &make_messages(2)).await.unwrap();
        store.save_session("b", &make_messages(2)).await.unwrap();

        store
            .append_context_receipts("a", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();
        store
            .append_context_receipts("b", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();

        assert_eq!(
            pairs(&store.list_context_receipts("a").await.unwrap()),
            [(1, 1, 1, 1)]
        );
        assert_eq!(
            pairs(&store.list_context_receipts("b").await.unwrap()),
            [(1, 1, 1, 1)]
        );
    }

    #[tokio::test]
    async fn cr11_saving_the_session_again_should_keep_its_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        store
            .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();

        store.save_session("s", &make_messages(4)).await.unwrap(); // upsert

        assert_eq!(store.list_context_receipts("s").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cr11_deleting_a_session_should_delete_its_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        store
            .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();

        store.delete_session("s").await.unwrap();

        assert!(store.list_context_receipts("s").await.unwrap().is_empty());
        // A new session with the same name starts clean.
        store.save_session("s", &make_messages(2)).await.unwrap();
        assert!(store.list_context_receipts("s").await.unwrap().is_empty());
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM context_receipts")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(rows.0, 0, "no orphaned rows");
    }

    #[tokio::test]
    async fn cr11_renaming_a_session_should_keep_its_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("old", &make_messages(2)).await.unwrap();
        store
            .append_context_receipts("old", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();

        store.rename_session("old", "new").await.unwrap();

        assert_eq!(store.list_context_receipts("new").await.unwrap().len(), 1);
        assert!(store.list_context_receipts("old").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cr11_appending_to_an_unknown_session_should_error() {
        let (store, _d) = open_test_store().await;
        let r = store
            .append_context_receipts("nope", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn cr11_an_old_database_should_gain_the_table_in_place() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        // A database made by an older hone: only `cli_sessions`.
        {
            let opts = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
            sqlx::query(
                "CREATE TABLE cli_sessions (id TEXT PRIMARY KEY, name TEXT UNIQUE NOT NULL,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, messages TEXT NOT NULL)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO cli_sessions VALUES ('id-1', 'old', 1, 1, '[]')")
                .execute(&pool)
                .await
                .unwrap();
        }

        let store = SessionStore::open(&path).await.unwrap();

        assert!(
            store.load_session("old").await.unwrap().is_some(),
            "old rows survive"
        );
        assert!(store.list_context_receipts("old").await.unwrap().is_empty());
        store
            .append_context_receipts("old", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();
        assert_eq!(store.list_context_receipts("old").await.unwrap().len(), 1);
    }

    // ---- CR21 of docs/plans/context-receipts-spec.md ------------------------------------
    //
    // ASSUMED API (the spec names the type but not its home): `hone_core::ReceiptBatch`
    // with public fields `agent_id: String` and `items: Vec<(u64, ContextReceipt)>`
    // (ordinal, receipt). `append_context_receipts(&self, session_name: &str,
    // batch: &hone_core::ReceiptBatch) -> Result<()>`. A run is one (store instance,
    // session, agent id); `seq` is the ordinal.

    #[tokio::test]
    async fn cr21_a_gap_in_ordinals_should_still_store_later_receipts_with_jumping_seq() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        // The agent dropped ordinals 1..=9_999 (the cap) before the first save.
        store
            .append_context_receipts("s", &batch("a1", 10_000, &[cr11_receipt(5, 1)]))
            .await
            .unwrap();
        // Later: ordinals 10_001 and 10_002 exist.
        store
            .append_context_receipts(
                "s",
                &batch("a1", 10_000, &[cr11_receipt(5, 1), cr11_receipt(5, 2), cr11_receipt(6, 1)]),
            )
            .await
            .unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(
            pairs(&got),
            [(1, 10_000, 5, 1), (1, 10_001, 5, 2), (1, 10_002, 6, 1)],
            "seq is the ordinal; nothing is silently dropped after the cap"
        );
    }

    #[tokio::test]
    async fn cr21_a_gap_between_two_saves_should_not_drop_the_new_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        store
            .append_context_receipts("s", &batch("a1", 1, &[cr11_receipt(1, 1)]))
            .await
            .unwrap();
        // Ordinals 2..=20 were dropped by the agent before this save.
        store
            .append_context_receipts("s", &batch("a1", 21, &[cr11_receipt(9, 1), cr11_receipt(9, 2)]))
            .await
            .unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(pairs(&got), [(1, 1, 1, 1), (1, 21, 9, 1), (1, 22, 9, 2)]);
    }

    #[tokio::test]
    async fn cr21_a_second_agent_id_in_the_same_store_should_start_a_new_run() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        store
            .append_context_receipts("s", &batch("agent-1", 1, &[cr11_receipt(1, 1), cr11_receipt(1, 2)]))
            .await
            .unwrap();

        // A fresh agent (e.g. after /new or a session switch) saving under the same
        // session: ordinals restart at 1; its first receipts must not be skipped.
        store
            .append_context_receipts("s", &batch("agent-2", 1, &[cr11_receipt(1, 1), cr11_receipt(1, 2)]))
            .await
            .unwrap();
        store
            .append_context_receipts("s", &batch("agent-2", 3, &[cr11_receipt(2, 1)]))
            .await
            .unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(
            pairs(&got),
            [(1, 1, 1, 1), (1, 2, 1, 2), (2, 1, 1, 1), (2, 2, 1, 2), (2, 3, 2, 1)]
        );
    }

    #[tokio::test]
    async fn cr21_overlapping_batches_should_not_duplicate_receipts() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();
        let all = [cr11_receipt(1, 1), cr11_receipt(1, 2), cr11_receipt(2, 1)];
        store
            .append_context_receipts("s", &batch("a1", 1, &all[..2]))
            .await
            .unwrap();
        // Overlaps ordinals 1-2 and adds 3; then the very same batch again.
        store.append_context_receipts("s", &batch("a1", 1, &all)).await.unwrap();
        store.append_context_receipts("s", &batch("a1", 1, &all)).await.unwrap();
        // A batch entirely behind what is stored changes nothing.
        store.append_context_receipts("s", &batch("a1", 2, &all[1..2])).await.unwrap();

        let got = store.list_context_receipts("s").await.unwrap();
        assert_eq!(pairs(&got), [(1, 1, 1, 1), (1, 2, 1, 2), (1, 3, 2, 1)]);
    }

    #[tokio::test]
    async fn cr21_an_empty_batch_should_store_nothing_and_not_error() {
        let (store, _d) = open_test_store().await;
        store.save_session("s", &make_messages(2)).await.unwrap();

        store.append_context_receipts("s", &batch("a1", 1, &[])).await.unwrap();

        assert!(store.list_context_receipts("s").await.unwrap().is_empty());
    }

    // ---- CR22 ---------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cr22_concurrent_first_saves_from_two_stores_should_get_distinct_runs() {
        for round in 0..8 {
            let dir = tempdir().unwrap();
            let path = dir.path().join("sessions.db");
            let a = std::sync::Arc::new(SessionStore::open(&path).await.unwrap());
            let b = std::sync::Arc::new(SessionStore::open(&path).await.unwrap());
            a.save_session("s", &make_messages(2)).await.unwrap();

            let (a2, b2) = (a.clone(), b.clone());
            let ta = tokio::spawn(async move {
                a2.append_context_receipts(
                    "s",
                    &batch("agent-a", 1, &[cr11_receipt(1, 1), cr11_receipt(1, 2)]),
                )
                .await
            });
            let tb = tokio::spawn(async move {
                b2.append_context_receipts(
                    "s",
                    &batch("agent-b", 1, &[cr11_receipt(2, 1), cr11_receipt(2, 2)]),
                )
                .await
            });
            ta.await.unwrap().unwrap_or_else(|e| panic!("round {round}: store A failed: {e:#}"));
            tb.await.unwrap().unwrap_or_else(|e| panic!("round {round}: store B failed: {e:#}"));

            let got = a.list_context_receipts("s").await.unwrap();
            let runs: std::collections::BTreeSet<u32> = got.iter().map(|r| r.run).collect();
            assert_eq!(runs, [1, 2].into(), "round {round}: distinct runs 1 and 2");
            assert_eq!(got.len(), 4, "round {round}: no lost rows");
            for run in [1, 2] {
                let seqs: Vec<u32> = got.iter().filter(|r| r.run == run).map(|r| r.seq).collect();
                assert_eq!(seqs, [1, 2], "round {round}: run {run} kept both receipts");
            }
            // The two writers' receipts were not mixed into one run.
            let turns_of = |run: u32| -> std::collections::BTreeSet<u32> {
                got.iter().filter(|r| r.run == run).map(|r| r.receipt.turn).collect()
            };
            assert_eq!(turns_of(1).len(), 1, "round {round}");
            assert_eq!(turns_of(2).len(), 1, "round {round}");
        }
    }
}
