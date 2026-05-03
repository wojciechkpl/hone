use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::runner::TaskOutcome;

/// One stored eval run plus its provenance metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalRunRecord {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    /// Optional git commit sha at the time of the run (e.g. from `git rev-parse HEAD`).
    pub commit_sha: Option<String>,
    /// Model id used (e.g. "anthropic:claude-sonnet-4-5").
    pub model: String,
    /// Hash of the system prompt + skeleton + tool registry contents at run time.
    pub config_hash: String,
    /// Optional human label (e.g. "nightly", "after-prompt-rewrite-v3").
    pub tag: Option<String>,
    /// All task outcomes for this run, in run order.
    pub outcomes: Vec<TaskOutcome>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub commit_sha: Option<String>,
    pub model: String,
    pub config_hash: String,
    pub tag: Option<String>,
    pub passed: usize,
    pub total: usize,
}

pub struct EvalRunStore {
    pool: SqlitePool,
}

impl EvalRunStore {
    pub async fn open(path: &Path) -> Result<Self, EvalHistoryError> {
        let url = format!(
            "sqlite://{}?mode=rwc",
            path.to_str().unwrap_or_default()
        );
        Self::connect(&url).await
    }

    pub async fn open_in_memory() -> Result<Self, EvalHistoryError> {
        Self::connect("sqlite::memory:").await
    }

    async fn connect(url: &str) -> Result<Self, EvalHistoryError> {
        let pool = SqlitePool::connect(url).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS eval_runs (
                id          TEXT    PRIMARY KEY,
                timestamp   INTEGER NOT NULL,
                commit_sha  TEXT,
                model       TEXT    NOT NULL,
                config_hash TEXT    NOT NULL,
                tag         TEXT,
                outcomes    TEXT    NOT NULL,
                passed      INTEGER NOT NULL DEFAULT 0,
                total       INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_eval_runs_timestamp ON eval_runs(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_eval_runs_tag ON eval_runs(tag);",
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    /// Persist one run. Generates a uuid v7 id and returns the new record.
    pub async fn record(
        &self,
        commit_sha: Option<String>,
        model: String,
        config_hash: String,
        tag: Option<String>,
        outcomes: Vec<TaskOutcome>,
    ) -> Result<EvalRunRecord, EvalHistoryError> {
        let id = Uuid::now_v7().to_string();
        let timestamp = Utc::now();
        let timestamp_micros = timestamp.timestamp_micros();

        let passed = outcomes.iter().filter(|o| matches!(o.status, crate::runner::TaskStatus::Pass)).count();
        let total = outcomes.len();

        let outcomes_json = serde_json::to_string(&outcomes)?;

        sqlx::query(
            "INSERT INTO eval_runs (id, timestamp, commit_sha, model, config_hash, tag, outcomes, passed, total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&id)
        .bind(timestamp_micros)
        .bind(&commit_sha)
        .bind(&model)
        .bind(&config_hash)
        .bind(&tag)
        .bind(&outcomes_json)
        .bind(passed as i64)
        .bind(total as i64)
        .execute(&self.pool)
        .await?;

        Ok(EvalRunRecord {
            id,
            timestamp,
            commit_sha,
            model,
            config_hash,
            tag,
            outcomes,
        })
    }

    /// Look up by id.
    pub async fn get(&self, id: &str) -> Result<Option<EvalRunRecord>, EvalHistoryError> {
        let row: Option<(String, i64, Option<String>, String, String, Option<String>, String)> =
            sqlx::query_as(
                "SELECT id, timestamp, commit_sha, model, config_hash, tag, outcomes
                 FROM eval_runs WHERE id = ?1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|(id, ts_micros, commit_sha, model, config_hash, tag, outcomes_json)| {
            let timestamp = Utc
                .timestamp_micros(ts_micros)
                .single()
                .unwrap_or_else(Utc::now);
            let outcomes: Vec<TaskOutcome> = serde_json::from_str(&outcomes_json)?;
            Ok(EvalRunRecord {
                id,
                timestamp,
                commit_sha,
                model,
                config_hash,
                tag,
                outcomes,
            })
        })
        .transpose()
    }

    /// Lightweight list of summaries, newest first by timestamp. tag=None means no filter.
    pub async fn list(
        &self,
        tag: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunSummary>, EvalHistoryError> {
        let rows: Vec<(String, i64, Option<String>, String, String, Option<String>, i64, i64)> =
            if let Some(tag_val) = tag {
                sqlx::query_as(
                    "SELECT id, timestamp, commit_sha, model, config_hash, tag, passed, total
                     FROM eval_runs WHERE tag = ?1 ORDER BY timestamp DESC LIMIT ?2",
                )
                .bind(tag_val)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query_as(
                    "SELECT id, timestamp, commit_sha, model, config_hash, tag, passed, total
                     FROM eval_runs ORDER BY timestamp DESC LIMIT ?1",
                )
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            };

        let summaries = rows
            .into_iter()
            .map(|(id, ts_micros, commit_sha, model, config_hash, tag, passed, total)| {
                let timestamp = Utc
                    .timestamp_micros(ts_micros)
                    .single()
                    .unwrap_or_else(Utc::now);
                RunSummary {
                    id,
                    timestamp,
                    commit_sha,
                    model,
                    config_hash,
                    tag,
                    passed: passed as usize,
                    total: total as usize,
                }
            })
            .collect();

        Ok(summaries)
    }

    /// Most recent run with the given tag, or None.
    pub async fn latest_with_tag(&self, tag: &str) -> Result<Option<EvalRunRecord>, EvalHistoryError> {
        let row: Option<(String, i64, Option<String>, String, String, Option<String>, String)> =
            sqlx::query_as(
                "SELECT id, timestamp, commit_sha, model, config_hash, tag, outcomes
                 FROM eval_runs WHERE tag = ?1 ORDER BY timestamp DESC LIMIT 1",
            )
            .bind(tag)
            .fetch_optional(&self.pool)
            .await?;

        row.map(|(id, ts_micros, commit_sha, model, config_hash, tag, outcomes_json)| {
            let timestamp = Utc
                .timestamp_micros(ts_micros)
                .single()
                .unwrap_or_else(Utc::now);
            let outcomes: Vec<TaskOutcome> = serde_json::from_str(&outcomes_json)?;
            Ok(EvalRunRecord {
                id,
                timestamp,
                commit_sha,
                model,
                config_hash,
                tag,
                outcomes,
            })
        })
        .transpose()
    }

    /// Delete by id. Returns true if a row was removed.
    pub async fn remove(&self, id: &str) -> Result<bool, EvalHistoryError> {
        let result = sqlx::query("DELETE FROM eval_runs WHERE id = ?1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EvalHistoryError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("serialise outcomes: {0}")]
    Serialise(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{TaskOutcome, TaskStatus};
    use chrono::TimeZone;

    fn make_outcome(id: &str, status: TaskStatus) -> TaskOutcome {
        TaskOutcome {
            id: id.to_owned(),
            status,
            duration_ms: 0,
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            success_check_stdout: String::new(),
            success_check_stderr: String::new(),
            success_check_exit_code: None,
        }
    }

    async fn make_store() -> EvalRunStore {
        EvalRunStore::open_in_memory().await.expect("open_in_memory")
    }

    #[tokio::test]
    async fn record_assigns_uuid_v7_id() {
        let store = make_store().await;
        let rec = store
            .record(None, "model-x".into(), "hash-a".into(), None, vec![])
            .await
            .expect("record");
        // uuid v7 has the variant bits in the version nibble = '7'
        assert_eq!(&rec.id[14..15], "7", "expected uuid v7, got id={}", rec.id);
    }

    #[tokio::test]
    async fn record_persists_all_fields_including_outcomes() {
        let store = make_store().await;
        let outcomes = vec![
            make_outcome("T001", TaskStatus::Pass),
            make_outcome("T002", TaskStatus::Fail),
        ];
        let rec = store
            .record(
                Some("abc123".into()),
                "model-y".into(),
                "hash-b".into(),
                Some("nightly".into()),
                outcomes.clone(),
            )
            .await
            .expect("record");

        let fetched = store.get(&rec.id).await.expect("get").expect("some");
        assert_eq!(fetched.commit_sha, Some("abc123".into()));
        assert_eq!(fetched.model, "model-y");
        assert_eq!(fetched.config_hash, "hash-b");
        assert_eq!(fetched.tag, Some("nightly".into()));
        assert_eq!(fetched.outcomes.len(), 2);
        assert_eq!(fetched.outcomes[0].id, "T001");
        assert_eq!(fetched.outcomes[1].status, TaskStatus::Fail);
    }

    #[tokio::test]
    async fn record_with_no_commit_sha_round_trips() {
        let store = make_store().await;
        let rec = store
            .record(None, "m".into(), "h".into(), None, vec![])
            .await
            .expect("record");
        let fetched = store.get(&rec.id).await.expect("get").expect("some");
        assert_eq!(fetched.commit_sha, None);
    }

    #[tokio::test]
    async fn record_with_no_tag_round_trips() {
        let store = make_store().await;
        let rec = store
            .record(None, "m".into(), "h".into(), None, vec![])
            .await
            .expect("record");
        let fetched = store.get(&rec.id).await.expect("get").expect("some");
        assert_eq!(fetched.tag, None);
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_id() {
        let store = make_store().await;
        let result = store.get("does-not-exist").await.expect("get");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_round_trips_outcomes() {
        let store = make_store().await;
        let outcomes = vec![
            make_outcome("A", TaskStatus::Pass),
            make_outcome("B", TaskStatus::Timeout),
            make_outcome("C", TaskStatus::AgentError),
        ];
        let rec = store
            .record(None, "m".into(), "h".into(), None, outcomes.clone())
            .await
            .expect("record");
        let fetched = store.get(&rec.id).await.expect("get").expect("some");
        assert_eq!(fetched.outcomes.len(), 3);
        assert_eq!(fetched.outcomes[0].id, "A");
        assert_eq!(fetched.outcomes[1].status, TaskStatus::Timeout);
        assert_eq!(fetched.outcomes[2].status, TaskStatus::AgentError);
    }

    #[tokio::test]
    async fn list_no_filter_returns_summaries_newest_first() {
        let store = make_store().await;
        // Insert via raw SQL to control timestamps precisely.
        let outcomes_json = "[]";
        let older_ts = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap().timestamp_micros();
        let mid_ts   = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap().timestamp_micros();
        let newer_ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap().timestamp_micros();

        for (id, ts) in [("id-old", older_ts), ("id-mid", mid_ts), ("id-new", newer_ts)] {
            sqlx::query(
                "INSERT INTO eval_runs (id, timestamp, commit_sha, model, config_hash, tag, outcomes)
                 VALUES (?1, ?2, NULL, 'm', 'h', NULL, ?3)",
            )
            .bind(id)
            .bind(ts)
            .bind(outcomes_json)
            .execute(&store.pool)
            .await
            .expect("insert");
        }

        let summaries = store.list(None, 10).await.expect("list");
        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].id, "id-new");
        assert_eq!(summaries[1].id, "id-mid");
        assert_eq!(summaries[2].id, "id-old");
    }

    #[tokio::test]
    async fn list_with_tag_filter_returns_only_matching() {
        let store = make_store().await;
        store
            .record(None, "m".into(), "h".into(), Some("nightly".into()), vec![])
            .await
            .expect("record");
        store
            .record(None, "m".into(), "h".into(), Some("nightly".into()), vec![])
            .await
            .expect("record");
        store
            .record(None, "m".into(), "h".into(), Some("adhoc".into()), vec![])
            .await
            .expect("record");

        let summaries = store.list(Some("nightly"), 10).await.expect("list");
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|s| s.tag.as_deref() == Some("nightly")));
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let store = make_store().await;
        for _ in 0..5 {
            store
                .record(None, "m".into(), "h".into(), None, vec![])
                .await
                .expect("record");
        }
        let summaries = store.list(None, 3).await.expect("list");
        assert_eq!(summaries.len(), 3);
    }

    #[tokio::test]
    async fn latest_with_tag_returns_newest_match() {
        let store = make_store().await;
        let outcomes_json = "[]";
        let older_ts = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap().timestamp_micros();
        let newer_ts = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap().timestamp_micros();

        for (id, ts) in [("old-nightly", older_ts), ("new-nightly", newer_ts)] {
            sqlx::query(
                "INSERT INTO eval_runs (id, timestamp, commit_sha, model, config_hash, tag, outcomes)
                 VALUES (?1, ?2, NULL, 'm', 'h', 'nightly', ?3)",
            )
            .bind(id)
            .bind(ts)
            .bind(outcomes_json)
            .execute(&store.pool)
            .await
            .expect("insert");
        }

        let rec = store
            .latest_with_tag("nightly")
            .await
            .expect("latest_with_tag")
            .expect("some");
        assert_eq!(rec.id, "new-nightly");
    }

    #[tokio::test]
    async fn latest_with_tag_returns_none_when_unknown() {
        let store = make_store().await;
        let result = store
            .latest_with_tag("ghost-tag")
            .await
            .expect("latest_with_tag");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn remove_returns_true_when_present() {
        let store = make_store().await;
        let rec = store
            .record(None, "m".into(), "h".into(), None, vec![])
            .await
            .expect("record");
        let removed = store.remove(&rec.id).await.expect("remove");
        assert!(removed);
        assert!(store.get(&rec.id).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn remove_returns_false_when_absent() {
        let store = make_store().await;
        let removed = store.remove("ghost-id").await.expect("remove");
        assert!(!removed);
    }
}
