use std::path::Path;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

/// What to run when a schedule fires.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleAction {
    /// Run a built-in or project recipe by name.
    Recipe { name: String },
    /// Run a raw prompt.
    Prompt { text: String },
}

/// Last-run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LastStatus {
    NeverRun,
    Success,
    Failed,
    Running,
}

impl LastStatus {
    fn as_str(self) -> &'static str {
        match self {
            LastStatus::NeverRun => "neverrun",
            LastStatus::Success => "success",
            LastStatus::Failed => "failed",
            LastStatus::Running => "running",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "success" => LastStatus::Success,
            "failed" => LastStatus::Failed,
            "running" => LastStatus::Running,
            _ => LastStatus::NeverRun,
        }
    }
}

/// One row in the schedule table.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScheduleEntry {
    pub id: String,
    pub name: String,
    pub cron_expr: String,
    pub action: ScheduleAction,
    pub agent: Option<String>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: LastStatus,
    pub created_at: DateTime<Utc>,
}

/// SQLite-backed persistent schedule store.
pub struct ScheduleStore {
    pool: SqlitePool,
}

type Row = (String, String, String, String, Option<String>, Option<i64>, String, i64);

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("schedule with name '{0}' already exists")]
    NameExists(String),
    #[error("invalid cron expression '{expr}': {reason}")]
    InvalidCron { expr: String, reason: String },
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(String),
}

impl ScheduleStore {
    async fn connect(pool: SqlitePool) -> Result<Self, ScheduleError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS schedules (
                id          TEXT    PRIMARY KEY,
                name        TEXT    NOT NULL UNIQUE,
                cron_expr   TEXT    NOT NULL,
                action_json TEXT    NOT NULL,
                agent       TEXT,
                last_run_at INTEGER,
                last_status TEXT    NOT NULL,
                created_at  INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .map_err(|e| ScheduleError::Migration(e.to_string()))?;

        Ok(Self { pool })
    }

    /// Open or create the SQLite database at `path`. Runs migrations on
    /// connect.
    pub async fn open(path: &Path) -> Result<Self, ScheduleError> {
        let url = format!(
            "sqlite://{}?mode=rwc",
            path.to_str()
                .ok_or_else(|| ScheduleError::Migration("path is not valid UTF-8".to_owned()))?
        );
        let pool = SqlitePool::connect(&url).await?;
        Self::connect(pool).await
    }

    /// In-memory store for tests.
    pub async fn open_in_memory() -> Result<Self, ScheduleError> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        Self::connect(pool).await
    }

    /// Add a new entry. Errors with NameExists if `name` already exists.
    /// Validates cron_expr eagerly via `croner::Cron::new(...)`. Generates a
    /// uuid v7 id.
    pub async fn add(
        &self,
        name: &str,
        cron_expr: &str,
        action: ScheduleAction,
        agent: Option<String>,
    ) -> Result<ScheduleEntry, ScheduleError> {
        // Validate cron expression first.
        croner::Cron::new(cron_expr).parse().map_err(|e| ScheduleError::InvalidCron {
            expr: cron_expr.to_owned(),
            reason: e.to_string(),
        })?;

        // Check for name collision before insert to return a clean error.
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM schedules WHERE name = ?1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if existing.is_some() {
            return Err(ScheduleError::NameExists(name.to_owned()));
        }

        let id = uuid::Uuid::now_v7().to_string();
        let action_json = serde_json::to_string(&action)
            .map_err(|e| ScheduleError::Migration(e.to_string()))?;
        let created_at = Utc::now();
        let created_at_secs = created_at.timestamp();

        sqlx::query(
            "INSERT INTO schedules \
             (id, name, cron_expr, action_json, agent, last_run_at, last_status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
        )
        .bind(&id)
        .bind(name)
        .bind(cron_expr)
        .bind(&action_json)
        .bind(&agent)
        .bind(LastStatus::NeverRun.as_str())
        .bind(created_at_secs)
        .execute(&self.pool)
        .await?;

        Ok(ScheduleEntry {
            id,
            name: name.to_owned(),
            cron_expr: cron_expr.to_owned(),
            action,
            agent,
            last_run_at: None,
            last_status: LastStatus::NeverRun,
            created_at,
        })
    }

    /// List all entries, ordered by name asc.
    pub async fn list(&self) -> Result<Vec<ScheduleEntry>, ScheduleError> {
        let rows: Vec<Row> = sqlx::query_as(
                "SELECT id, name, cron_expr, action_json, agent, \
                 last_run_at, last_status, created_at \
                 FROM schedules ORDER BY name ASC",
            )
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter().map(row_to_entry).collect()
    }

    /// Look up by name. Returns Ok(None) when absent.
    pub async fn get_by_name(&self, name: &str) -> Result<Option<ScheduleEntry>, ScheduleError> {
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, name, cron_expr, action_json, agent, \
             last_run_at, last_status, created_at \
             FROM schedules WHERE name = ?1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_entry).transpose()
    }

    /// Delete by name. Returns true if a row was removed.
    pub async fn remove(&self, name: &str) -> Result<bool, ScheduleError> {
        let result = sqlx::query("DELETE FROM schedules WHERE name = ?1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Mark the entry as running (called when the tick loop is about to
    /// execute). Updates last_status only (last_run_at is set on completion).
    pub async fn mark_running(&self, id: &str) -> Result<(), ScheduleError> {
        sqlx::query("UPDATE schedules SET last_status = ?1 WHERE id = ?2")
            .bind(LastStatus::Running.as_str())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record completion. Updates last_run_at = `at` and last_status.
    pub async fn record_completion(
        &self,
        id: &str,
        success: bool,
        at: DateTime<Utc>,
    ) -> Result<(), ScheduleError> {
        let status = if success { LastStatus::Success } else { LastStatus::Failed };
        sqlx::query(
            "UPDATE schedules SET last_status = ?1, last_run_at = ?2 WHERE id = ?3",
        )
        .bind(status.as_str())
        .bind(at.timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Compute the next trigger time after `from` for `entry`. Returns None on
    /// invalid cron (shouldn't happen since add() validates).
    pub fn next_trigger_after(
        entry: &ScheduleEntry,
        from: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        croner::Cron::new(&entry.cron_expr)
            .parse()
            .ok()?
            .find_next_occurrence(&from, false)
            .ok()
    }

    /// Compute the minimum period of the cron expression by finding two
    /// consecutive occurrences from the Unix epoch. Returns `None` if the
    /// expression is invalid or period cannot be determined.
    fn cron_period(entry: &ScheduleEntry) -> Option<chrono::Duration> {
        let epoch = DateTime::from_timestamp(0, 0)?;
        let t1 = Self::next_trigger_after(entry, epoch)?;
        let t2 = Self::next_trigger_after(entry, t1)?;
        Some(t2 - t1)
    }

    /// Return all entries whose next trigger after `(last_run_at OR created_at)`
    /// is <= `now`. Excludes entries currently in LastStatus::Running.
    ///
    /// Due check: `base + period <= now`, where `period` is the cron's minimum
    /// interval computed from two consecutive triggers at epoch. This gives
    /// relative scheduling semantics — if the job ran at `base`, the next run
    /// is `base + period` — rather than clock-aligned absolute scheduling.
    pub async fn due_entries(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ScheduleEntry>, ScheduleError> {
        let all = self.list().await?;
        let mut due: Vec<ScheduleEntry> = all
            .into_iter()
            .filter(|e| e.last_status != LastStatus::Running)
            .filter(|e| {
                let base = e.last_run_at.unwrap_or(e.created_at);
                matches!(Self::cron_period(e), Some(period) if base + period <= now)
            })
            .collect();
        due.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(due)
    }
}

fn row_to_entry(row: Row) -> Result<ScheduleEntry, ScheduleError> {
    let (id, name, cron_expr, action_json, agent, last_run_at_secs, last_status_str, created_at_secs) =
        row;

    let action: ScheduleAction = serde_json::from_str(&action_json).map_err(|e| {
        ScheduleError::Migration(format!("corrupt action_json for '{name}': {e}"))
    })?;

    let last_run_at = last_run_at_secs.and_then(|s| DateTime::from_timestamp(s, 0));

    let created_at = DateTime::from_timestamp(created_at_secs, 0)
        .ok_or_else(|| ScheduleError::Migration(format!("corrupt created_at for '{name}'")))?;

    Ok(ScheduleEntry {
        id,
        name,
        cron_expr,
        action,
        agent,
        last_run_at,
        last_status: LastStatus::from_str(&last_status_str),
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_entry(cron_expr: &str) -> ScheduleEntry {
        ScheduleEntry {
            id: "test-id".to_owned(),
            name: "test".to_owned(),
            cron_expr: cron_expr.to_owned(),
            action: ScheduleAction::Prompt {
                text: "hello".to_owned(),
            },
            agent: None,
            last_run_at: None,
            last_status: LastStatus::NeverRun,
            created_at: Utc::now(),
        }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, m, s).unwrap()
    }

    // ── next_trigger_after (pure) ─────────────────────────────────────────────

    #[test]
    fn next_trigger_after_simple_daily_at_9am() {
        let entry = make_entry("0 9 * * *");
        let from = utc(2026, 5, 2, 8, 0, 0);
        let next = ScheduleStore::next_trigger_after(&entry, from).unwrap();
        assert_eq!(next, utc(2026, 5, 2, 9, 0, 0));
    }

    #[test]
    fn next_trigger_after_returns_following_day_when_past_time() {
        let entry = make_entry("0 9 * * *");
        let from = utc(2026, 5, 2, 10, 0, 0);
        let next = ScheduleStore::next_trigger_after(&entry, from).unwrap();
        assert_eq!(next, utc(2026, 5, 3, 9, 0, 0));
    }

    #[test]
    fn next_trigger_after_every_5_minutes() {
        let entry = make_entry("*/5 * * * *");
        let from = utc(2026, 5, 2, 12, 1, 0);
        let next = ScheduleStore::next_trigger_after(&entry, from).unwrap();
        assert_eq!(next, utc(2026, 5, 2, 12, 5, 0));
    }

    #[test]
    fn next_trigger_after_invalid_cron_returns_none() {
        let entry = make_entry("not a cron");
        let from = utc(2026, 5, 2, 12, 0, 0);
        let result = ScheduleStore::next_trigger_after(&entry, from);
        assert!(result.is_none());
    }

    // ── add / list / get / remove ─────────────────────────────────────────────

    #[tokio::test]
    async fn add_creates_entry_with_uuid_id() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "daily-audit",
                "0 9 * * *",
                ScheduleAction::Prompt {
                    text: "audit".to_owned(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(!entry.id.is_empty());

        let fetched = store.get_by_name("daily-audit").await.unwrap().unwrap();
        assert_eq!(fetched.id, entry.id);
    }

    #[tokio::test]
    async fn add_persists_all_fields() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let action = ScheduleAction::Prompt {
            text: "run security scan".to_owned(),
        };
        store
            .add("scan", "0 9 * * *", action.clone(), Some("sec-bot".to_owned()))
            .await
            .unwrap();

        let fetched = store.get_by_name("scan").await.unwrap().unwrap();
        assert_eq!(fetched.name, "scan");
        assert_eq!(fetched.cron_expr, "0 9 * * *");
        assert_eq!(fetched.action, action);
        assert_eq!(fetched.agent, Some("sec-bot".to_owned()));
        assert_eq!(fetched.last_status, LastStatus::NeverRun);
        assert!(fetched.last_run_at.is_none());
    }

    #[tokio::test]
    async fn add_with_recipe_action() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let action = ScheduleAction::Recipe {
            name: "tdd-feature".to_owned(),
        };
        let entry = store
            .add("tdd-run", "0 8 * * 1", action.clone(), None)
            .await
            .unwrap();
        assert_eq!(entry.action, action);
    }

    #[tokio::test]
    async fn add_with_prompt_action() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let action = ScheduleAction::Prompt {
            text: "audit security".to_owned(),
        };
        let entry = store
            .add("sec-prompt", "0 0 * * *", action.clone(), None)
            .await
            .unwrap();
        assert_eq!(entry.action, action);
    }

    #[tokio::test]
    async fn add_duplicate_name_errors() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        store
            .add(
                "dup",
                "0 9 * * *",
                ScheduleAction::Prompt {
                    text: "first".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        let err = store
            .add(
                "dup",
                "0 10 * * *",
                ScheduleAction::Prompt {
                    text: "second".to_owned(),
                },
                None,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScheduleError::NameExists(ref n) if n == "dup"),
            "expected NameExists, got {err:?}"
        );
    }

    #[tokio::test]
    async fn add_invalid_cron_errors() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let err = store
            .add(
                "bad-cron",
                "garbage",
                ScheduleAction::Prompt {
                    text: "noop".to_owned(),
                },
                None,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScheduleError::InvalidCron { .. }),
            "expected InvalidCron, got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_returns_entries_sorted_by_name_asc() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        for name in ["zebra", "alpha", "mango"] {
            store
                .add(
                    name,
                    "0 9 * * *",
                    ScheduleAction::Prompt {
                        text: name.to_owned(),
                    },
                    None,
                )
                .await
                .unwrap();
        }
        let entries = store.list().await.unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mango", "zebra"]);
    }

    #[tokio::test]
    async fn list_empty_returns_empty_vec() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entries = store.list().await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn get_by_name_returns_none_for_unknown() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let result = store.get_by_name("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn remove_returns_true_when_present() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        store
            .add(
                "to-remove",
                "0 9 * * *",
                ScheduleAction::Prompt {
                    text: "bye".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        let removed = store.remove("to-remove").await.unwrap();
        assert!(removed);
        let fetched = store.get_by_name("to-remove").await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn remove_returns_false_when_absent() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let removed = store.remove("ghost").await.unwrap();
        assert!(!removed);
    }

    // ── Status transitions ────────────────────────────────────────────────────

    #[tokio::test]
    async fn mark_running_sets_status_running() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "runner",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "go".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        store.mark_running(&entry.id).await.unwrap();

        let fetched = store.get_by_name("runner").await.unwrap().unwrap();
        assert_eq!(fetched.last_status, LastStatus::Running);
    }

    #[tokio::test]
    async fn record_completion_success_sets_last_run_at_and_status() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "completer",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "done".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        store.mark_running(&entry.id).await.unwrap();
        let at = utc(2026, 5, 2, 12, 5, 0);
        store.record_completion(&entry.id, true, at).await.unwrap();

        let fetched = store.get_by_name("completer").await.unwrap().unwrap();
        assert_eq!(fetched.last_status, LastStatus::Success);
        assert_eq!(fetched.last_run_at, Some(at));
    }

    #[tokio::test]
    async fn record_completion_failure_sets_status_failed() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "failer",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "oops".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        store.mark_running(&entry.id).await.unwrap();
        let at = utc(2026, 5, 2, 12, 5, 0);
        store.record_completion(&entry.id, false, at).await.unwrap();

        let fetched = store.get_by_name("failer").await.unwrap().unwrap();
        assert_eq!(fetched.last_status, LastStatus::Failed);
    }

    // ── due_entries ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn due_entries_returns_empty_when_nothing_due() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        // Created at 12:00, cron "0 9 * * *" — next trigger after 12:00 is 9 AM tomorrow.
        let _entry = store
            .add(
                "morning-job",
                "0 9 * * *",
                ScheduleAction::Prompt {
                    text: "morning".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        // Seed created_at to 12:00 so next_trigger_after(12:00) = 9 AM tomorrow.
        // The implementer stores created_at as Unix seconds; tests may reach into
        // the pool to back-date it, or the implementer exposes a back-door. For now
        // the test drives from the store's public API: the entry was JUST created,
        // so created_at ≈ now (2026-05-02T12:00 is simulated via `now` arg).
        let now = utc(2026, 5, 2, 12, 30, 0);
        // An entry created at real-now (~12:00 UTC) with cron "0 9 * * *" has its
        // next trigger at 09:00 the NEXT day, so it must NOT appear as due at 12:30.
        let due = store.due_entries(now).await.unwrap();
        assert!(due.is_empty(), "expected empty, got {due:?}");
    }

    #[tokio::test]
    async fn due_entries_returns_overdue_entries() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        // cron "*/5 * * * *": next trigger after created_at is within 5 minutes.
        // We pass `now` far enough ahead that the entry is definitely overdue.
        let _entry = store
            .add(
                "five-min",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "tick".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        // 10 minutes from now: definitely past next trigger.
        let now = Utc::now() + chrono::Duration::minutes(10);
        let due = store.due_entries(now).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "five-min");
    }

    #[tokio::test]
    async fn due_entries_skips_currently_running_entries() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "running-job",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "tick".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        store.mark_running(&entry.id).await.unwrap();

        let now = Utc::now() + chrono::Duration::minutes(10);
        let due = store.due_entries(now).await.unwrap();
        assert!(due.is_empty(), "running entries must be excluded");
    }

    #[tokio::test]
    async fn due_entries_uses_last_run_at_when_present() {
        let store = ScheduleStore::open_in_memory().await.unwrap();
        let entry = store
            .add(
                "last-run-job",
                "*/5 * * * *",
                ScheduleAction::Prompt {
                    text: "tick".to_owned(),
                },
                None,
            )
            .await
            .unwrap();

        // Record a completed run 5 minutes ago.
        let last_run_at = Utc::now() - chrono::Duration::minutes(5);
        store.mark_running(&entry.id).await.unwrap();
        store
            .record_completion(&entry.id, true, last_run_at)
            .await
            .unwrap();

        // 3 minutes after last_run_at: next trigger is last_run_at + 5 min → NOT due.
        let now_before = last_run_at + chrono::Duration::minutes(3);
        let due = store.due_entries(now_before).await.unwrap();
        assert!(
            due.is_empty(),
            "3 min after last_run_at should not be due (next trigger is +5 min)"
        );

        // 6 minutes after last_run_at: next trigger is last_run_at + 5 min → due.
        let now_after = last_run_at + chrono::Duration::minutes(6);
        let due = store.due_entries(now_after).await.unwrap();
        assert_eq!(due.len(), 1, "6 min after last_run_at should be due");
    }
}
