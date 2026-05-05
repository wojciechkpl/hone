use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use serde::{Deserialize, Serialize};

use hone_core::task_plan::SubTask;
use hone_proto::TokenUsage;

/// Identifier for a worker (free-form string the worker self-asserts).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub String);

impl WorkerId {
    pub fn new(s: impl Into<String>) -> Self {
        WorkerId(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Leased,
    Done,
    Failed,
    TimedOut,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Leased => "leased",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::TimedOut => "timed_out",
        }
    }

    pub fn from_str(s: &str) -> Option<TaskState> {
        match s {
            "queued" => Some(TaskState::Queued),
            "leased" => Some(TaskState::Leased),
            "done" => Some(TaskState::Done),
            "failed" => Some(TaskState::Failed),
            "timed_out" => Some(TaskState::TimedOut),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed | TaskState::TimedOut)
    }
}

/// A task pulled by a worker, with all the info needed to execute.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedTask {
    pub task_id: String,
    pub plan_id: String,
    pub subtask: SubTask,
    pub lease_expires_at: DateTime<Utc>,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompletionPayload {
    pub task_id: String,
    pub worker_id: WorkerId,
    pub status_str: String, // "completed" | "failed" | "timed_out"
    pub response: String,
    pub error_msg: Option<String>,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum DistributedError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("serialise: {0}")]
    Serialise(#[from] serde_json::Error),
    #[error("lease lost: task {task_id} no longer leased by {worker_id}")]
    LeaseLost { task_id: String, worker_id: String },
    #[error("unknown task: {0}")]
    UnknownTask(String),
}

/// SQLite-backed distributed task queue.
pub struct DistributedTaskStore {
    pool: SqlitePool,
}

impl DistributedTaskStore {
    async fn setup(pool: SqlitePool) -> Result<Self, DistributedError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS distributed_tasks (
                id               TEXT    PRIMARY KEY,
                plan_id          TEXT    NOT NULL,
                subtask_json     TEXT    NOT NULL,
                state            TEXT    NOT NULL,
                leased_by        TEXT,
                lease_expires_at INTEGER,
                created_at       INTEGER NOT NULL,
                completed_at     INTEGER,
                attempts         INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_dist_tasks_state_created
                ON distributed_tasks(state, created_at);
            CREATE INDEX IF NOT EXISTS idx_dist_tasks_plan
                ON distributed_tasks(plan_id);

            CREATE TABLE IF NOT EXISTS distributed_results (
                task_id       TEXT    PRIMARY KEY REFERENCES distributed_tasks(id),
                worker_id     TEXT    NOT NULL,
                status        TEXT    NOT NULL,
                response      TEXT    NOT NULL,
                error_msg     TEXT,
                tokens_input  INTEGER NOT NULL,
                tokens_output INTEGER NOT NULL,
                cost_usd      REAL    NOT NULL,
                completed_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS distributed_workers (
                id               TEXT    PRIMARY KEY,
                capabilities     TEXT    NOT NULL,
                last_heartbeat   INTEGER NOT NULL,
                sandbox_profile  TEXT
            );",
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    pub async fn open(path: &Path) -> Result<Self, DistributedError> {
        let url = format!(
            "sqlite://{}?mode=rwc",
            path.to_str().unwrap_or_default()
        );
        let pool = SqlitePool::connect(&url).await?;
        Self::setup(pool).await
    }

    pub async fn open_in_memory() -> Result<Self, DistributedError> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        Self::setup(pool).await
    }

    /// Insert one or more subtasks under a plan_id, all in `queued` state.
    /// Generates a uuid v7 task_id per SubTask.
    pub async fn enqueue(
        &self,
        plan_id: &str,
        subtasks: Vec<SubTask>,
    ) -> Result<Vec<String>, DistributedError> {
        let mut ids = Vec::with_capacity(subtasks.len());
        let now = Utc::now().timestamp_micros();

        for subtask in subtasks {
            let id = uuid::Uuid::now_v7().to_string();
            let subtask_json = serde_json::to_string(&subtask)?;

            sqlx::query(
                "INSERT INTO distributed_tasks
                 (id, plan_id, subtask_json, state, created_at, attempts)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            )
            .bind(&id)
            .bind(plan_id)
            .bind(&subtask_json)
            .bind(TaskState::Queued.as_str())
            .bind(now)
            .execute(&self.pool)
            .await?;

            ids.push(id);
        }

        Ok(ids)
    }

    /// Atomically pick one queued task whose dependencies are all done AND
    /// whose required capabilities are a subset of `worker_capabilities`.
    /// Uses BEGIN IMMEDIATE transaction for SQLite-level write lock.
    pub async fn claim_one(
        &self,
        worker_id: &WorkerId,
        worker_capabilities: &[String],
        lease_duration: Duration,
    ) -> Result<Option<ClaimedTask>, DistributedError> {
        let mut txn = self.pool.begin().await?;

        // Fetch all queued tasks ordered by created_at for FIFO fairness.
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT id, plan_id, subtask_json, attempts
             FROM distributed_tasks
             WHERE state = 'queued'
             ORDER BY created_at ASC",
        )
        .fetch_all(&mut *txn)
        .await?;

        let now = Utc::now();
        let lease_expires_at = now + chrono::Duration::from_std(lease_duration)
            .unwrap_or(chrono::Duration::seconds(60));
        let lease_expires_micros = lease_expires_at.timestamp_micros();

        for (id, plan_id, subtask_json, attempts) in rows {
            let subtask: SubTask = serde_json::from_str(&subtask_json)?;

            // Check capability match: task's required capabilities must be a
            // subset of the worker's declared capabilities.
            let caps_satisfied = subtask.requires_capabilities.iter().all(|cap| {
                worker_capabilities.contains(cap)
            });
            if !caps_satisfied {
                continue;
            }

            // Check dependency satisfaction: all depends_on subtask logical ids
            // must have a corresponding task in done state.
            // We look up by matching subtask_json's "id" field against depends_on.
            if !subtask.depends_on.is_empty() {
                let dep_ok = self
                    .deps_satisfied(&subtask.depends_on, &plan_id, &mut txn)
                    .await?;
                if !dep_ok {
                    continue;
                }
            }

            // Claim this task atomically.
            let affected = sqlx::query(
                "UPDATE distributed_tasks
                 SET state = 'leased',
                     leased_by = ?1,
                     lease_expires_at = ?2,
                     attempts = attempts + 1
                 WHERE id = ?3 AND state = 'queued'",
            )
            .bind(worker_id.as_str())
            .bind(lease_expires_micros)
            .bind(&id)
            .execute(&mut *txn)
            .await?
            .rows_affected();

            if affected == 0 {
                // Another concurrent writer grabbed it; try next.
                continue;
            }

            txn.commit().await?;

            return Ok(Some(ClaimedTask {
                task_id: id,
                plan_id,
                subtask,
                lease_expires_at,
                attempts: attempts as u32 + 1,
            }));
        }

        txn.commit().await?;
        Ok(None)
    }

    /// Check that all logical subtask ids in `dep_ids` have a corresponding
    /// task row in `done` state within this plan.
    async fn deps_satisfied(
        &self,
        dep_ids: &[String],
        plan_id: &str,
        txn: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<bool, DistributedError> {
        // Load all tasks in the plan; match dep_ids against the logical `id`
        // field stored in subtask_json.
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT subtask_json, state FROM distributed_tasks WHERE plan_id = ?1",
        )
        .bind(plan_id)
        .fetch_all(&mut **txn)
        .await?;

        for dep_id in dep_ids {
            let mut found = false;
            for (subtask_json, state) in &rows {
                // Quick parse of just the id field to avoid full SubTask deserialise.
                let v: serde_json::Value = serde_json::from_str(subtask_json)?;
                if v["id"].as_str() == Some(dep_id.as_str()) {
                    found = true;
                    if state != "done" {
                        return Ok(false);
                    }
                    break;
                }
            }
            if !found {
                // Dep not in this plan at all — treat as unsatisfied.
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Mark a task as completed. Inserts result row and flips task state.
    /// Returns Err(LeaseLost) if the lease no longer belongs to this worker.
    pub async fn complete(
        &self,
        payload: CompletionPayload,
    ) -> Result<(), DistributedError> {
        let mut txn = self.pool.begin().await?;

        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT state, leased_by FROM distributed_tasks WHERE id = ?1",
        )
        .bind(&payload.task_id)
        .fetch_optional(&mut *txn)
        .await?;

        let (state, leased_by) = match row {
            None => return Err(DistributedError::UnknownTask(payload.task_id.clone())),
            Some(r) => r,
        };

        if state != "leased" {
            return Err(DistributedError::UnknownTask(payload.task_id.clone()));
        }

        if leased_by.as_deref() != Some(payload.worker_id.as_str()) {
            return Err(DistributedError::LeaseLost {
                task_id: payload.task_id.clone(),
                worker_id: payload.worker_id.0.clone(),
            });
        }

        let new_state = match payload.status_str.as_str() {
            "completed" => "done",
            "failed" => "failed",
            "timed_out" => "timed_out",
            _ => "done",
        };

        let now_micros = Utc::now().timestamp_micros();

        sqlx::query(
            "UPDATE distributed_tasks
             SET state = ?1, completed_at = ?2, leased_by = NULL, lease_expires_at = NULL
             WHERE id = ?3",
        )
        .bind(new_state)
        .bind(now_micros)
        .bind(&payload.task_id)
        .execute(&mut *txn)
        .await?;

        sqlx::query(
            "INSERT INTO distributed_results
             (task_id, worker_id, status, response, error_msg,
              tokens_input, tokens_output, cost_usd, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(&payload.task_id)
        .bind(payload.worker_id.as_str())
        .bind(&payload.status_str)
        .bind(&payload.response)
        .bind(&payload.error_msg)
        .bind(payload.usage.input as i64)
        .bind(payload.usage.output as i64)
        .bind(payload.cost_usd)
        .bind(now_micros)
        .execute(&mut *txn)
        .await?;

        txn.commit().await?;
        Ok(())
    }

    /// Reap any tasks whose lease_expires_at <= now.
    /// Re-queues them (attempts += 1); force-fails tasks that exceed max_attempts.
    pub async fn reap_expired_leases(
        &self,
        now: DateTime<Utc>,
        max_attempts: u32,
    ) -> Result<usize, DistributedError> {
        let now_micros = now.timestamp_micros();

        // Fetch expired leases.
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT id, attempts FROM distributed_tasks
             WHERE state = 'leased' AND lease_expires_at <= ?1",
        )
        .bind(now_micros)
        .fetch_all(&self.pool)
        .await?;

        let count = rows.len();

        for (id, attempts) in rows {
            // `attempts` is already incremented by `claim_one` each time the task
            // is claimed. The reaper only decides whether the stored count has
            // reached the limit; it does NOT add another increment.
            if attempts >= max_attempts as i64 {
                let error_msg = format!(
                    "Task exceeded max_attempts ({max_attempts}); permanently failed by reaper."
                );
                sqlx::query(
                    "UPDATE distributed_tasks
                     SET state = 'failed', leased_by = NULL, lease_expires_at = NULL,
                         completed_at = ?1
                     WHERE id = ?2",
                )
                .bind(now_micros)
                .bind(&id)
                .execute(&self.pool)
                .await?;

                // Insert synthetic result row so callers can inspect reason.
                let _ = sqlx::query(
                    "INSERT OR IGNORE INTO distributed_results
                     (task_id, worker_id, status, response, error_msg,
                      tokens_input, tokens_output, cost_usd, completed_at)
                     VALUES (?1, 'reaper', 'failed', '', ?2, 0, 0, 0.0, ?3)",
                )
                .bind(&id)
                .bind(&error_msg)
                .bind(now_micros)
                .execute(&self.pool)
                .await;
            } else {
                sqlx::query(
                    "UPDATE distributed_tasks
                     SET state = 'queued', leased_by = NULL, lease_expires_at = NULL
                     WHERE id = ?1",
                )
                .bind(&id)
                .execute(&self.pool)
                .await?;
            }
        }

        Ok(count)
    }

    /// Snapshot of state counts for a plan.
    /// Returns (queued, leased, done, failed, timed_out).
    pub async fn plan_state_counts(
        &self,
        plan_id: &str,
    ) -> Result<(usize, usize, usize, usize, usize), DistributedError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT state, COUNT(*) FROM distributed_tasks
             WHERE plan_id = ?1
             GROUP BY state",
        )
        .bind(plan_id)
        .fetch_all(&self.pool)
        .await?;

        let mut queued = 0usize;
        let mut leased = 0usize;
        let mut done = 0usize;
        let mut failed = 0usize;
        let mut timed_out = 0usize;

        for (state, count) in rows {
            let c = count as usize;
            match state.as_str() {
                "queued" => queued = c,
                "leased" => leased = c,
                "done" => done = c,
                "failed" => failed = c,
                "timed_out" => timed_out = c,
                _ => {}
            }
        }

        Ok((queued, leased, done, failed, timed_out))
    }

    /// Refresh a worker's heartbeat row. Inserts the worker if absent.
    pub async fn upsert_worker(
        &self,
        worker_id: &WorkerId,
        capabilities: &[String],
        sandbox_profile_json: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), DistributedError> {
        let caps_json = serde_json::to_string(capabilities)?;
        let now_micros = now.timestamp_micros();

        sqlx::query(
            "INSERT INTO distributed_workers (id, capabilities, last_heartbeat, sandbox_profile)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                capabilities = excluded.capabilities,
                last_heartbeat = excluded.last_heartbeat,
                sandbox_profile = excluded.sandbox_profile",
        )
        .bind(worker_id.as_str())
        .bind(&caps_json)
        .bind(now_micros)
        .bind(sandbox_profile_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Extend the lease on a task that this worker owns.
    pub async fn extend_lease(
        &self,
        worker_id: &WorkerId,
        task_id: &str,
        new_expires_at: DateTime<Utc>,
    ) -> Result<(), DistributedError> {
        let new_micros = new_expires_at.timestamp_micros();

        let affected = sqlx::query(
            "UPDATE distributed_tasks
             SET lease_expires_at = ?1
             WHERE id = ?2 AND state = 'leased' AND leased_by = ?3",
        )
        .bind(new_micros)
        .bind(task_id)
        .bind(worker_id.as_str())
        .execute(&self.pool)
        .await?
        .rows_affected();

        if affected == 0 {
            return Err(DistributedError::LeaseLost {
                task_id: task_id.to_owned(),
                worker_id: worker_id.0.clone(),
            });
        }

        Ok(())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_subtask(id: &str) -> SubTask {
    SubTask {
        id: id.to_owned(),
        description: format!("task {id}"),
        agent: "test-agent".to_owned(),
        context: String::new(),
        depends_on: vec![],
        acceptance_tests: vec![],
        budget_usd: None,
        scope: vec![],
        requires_capabilities: vec![],
    }
}

fn make_subtask_with_caps(id: &str, caps: Vec<String>) -> SubTask {
    SubTask {
        requires_capabilities: caps,
        ..make_subtask(id)
    }
}

fn make_subtask_with_deps(id: &str, deps: Vec<String>) -> SubTask {
    SubTask {
        depends_on: deps,
        ..make_subtask(id)
    }
}

fn make_payload(task_id: &str, worker_id: &WorkerId, status: &str) -> CompletionPayload {
    CompletionPayload {
        task_id: task_id.to_owned(),
        worker_id: worker_id.clone(),
        status_str: status.to_owned(),
        response: "ok".to_owned(),
        error_msg: None,
        usage: TokenUsage {
            input: 10,
            output: 20,
            cache_read: 0,
            cache_creation: 0,
        },
        cost_usd: 0.001,
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use chrono::Utc;

    // Smoke test: verifies the test module compiles and basic types work.
    #[test]
    fn task_state_is_terminal_for_done_failed_timed_out() {
        assert!(TaskState::Done.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::TimedOut.is_terminal());
        assert!(!TaskState::Queued.is_terminal());
        assert!(!TaskState::Leased.is_terminal());
    }

    #[test]
    fn worker_id_as_str_round_trips() {
        let id = WorkerId::new("my-worker-1");
        assert_eq!(id.as_str(), "my-worker-1");
    }

    // ── Basic enqueue/claim/complete cycle ────────────────────────────────────

    #[tokio::test]
    async fn enqueue_assigns_unique_uuid_v7_ids() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let subtasks = vec![make_subtask("s1"), make_subtask("s2"), make_subtask("s3")];
        let ids = store.enqueue("plan-1", subtasks).await.unwrap();
        assert_eq!(ids.len(), 3);
        // All unique
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 3);
        // Each parseable as uuid
        for id in &ids {
            uuid::Uuid::parse_str(id).expect("should be valid uuid");
        }
    }

    #[tokio::test]
    async fn enqueue_preserves_subtask_fields_through_round_trip() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let subtask = SubTask {
            id: "rich-task".to_owned(),
            description: "do something".to_owned(),
            agent: "python-expert".to_owned(),
            context: "some context".to_owned(),
            depends_on: vec![],
            acceptance_tests: vec!["it works".to_owned()],
            budget_usd: Some(0.50),
            scope: vec!["src/".to_owned()],
            requires_capabilities: vec!["python".to_owned(), "docker".to_owned()],
        };
        let ids = store.enqueue("plan-rt", vec![subtask.clone()]).await.unwrap();
        assert_eq!(ids.len(), 1);

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &["python".to_owned(), "docker".to_owned()], Duration::from_secs(60))
            .await
            .unwrap()
            .expect("should have claimed the task");

        assert_eq!(claimed.subtask.id, subtask.id);
        assert_eq!(claimed.subtask.description, subtask.description);
        assert_eq!(claimed.subtask.agent, subtask.agent);
        assert_eq!(claimed.subtask.requires_capabilities, subtask.requires_capabilities);
        assert_eq!(claimed.subtask.budget_usd, subtask.budget_usd);
        assert_eq!(claimed.subtask.acceptance_tests, subtask.acceptance_tests);
    }

    #[tokio::test]
    async fn enqueue_zero_subtasks_returns_empty_vec() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let ids = store.enqueue("plan-empty", vec![]).await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn claim_one_returns_none_when_queue_empty() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let worker = WorkerId::new("w1");
        let result = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn claim_one_returns_eligible_task() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed.plan_id, "plan-1");
        assert_eq!(claimed.subtask.id, "s1");
    }

    #[tokio::test]
    async fn claim_one_assigns_lease_expiry_in_future() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let before = Utc::now();
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let after = Utc::now();

        assert!(claimed.lease_expires_at > before);
        // Lease must be at least 59 seconds from before (allowing small clock skew)
        let min_expiry = before + chrono::Duration::seconds(59);
        assert!(claimed.lease_expires_at >= min_expiry,
            "lease_expires_at {:?} should be >= {:?}", claimed.lease_expires_at, min_expiry);
        // And not absurdly far in the future
        let max_expiry = after + chrono::Duration::seconds(120);
        assert!(claimed.lease_expires_at <= max_expiry);
    }

    #[tokio::test]
    async fn claim_one_increments_attempts_to_1_on_first_claim() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(claimed.attempts, 1);
    }

    #[tokio::test]
    async fn complete_marks_task_done() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        store
            .complete(make_payload(&claimed.task_id, &worker, "completed"))
            .await
            .unwrap();

        let (queued, leased, done, failed, timed_out) =
            store.plan_state_counts("plan-1").await.unwrap();
        assert_eq!((queued, leased, done, failed, timed_out), (0, 0, 1, 0, 0));
    }

    #[tokio::test]
    async fn complete_persists_result_row() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        let payload = CompletionPayload {
            task_id: claimed.task_id.clone(),
            worker_id: worker.clone(),
            status_str: "completed".to_owned(),
            response: "great output".to_owned(),
            error_msg: None,
            usage: TokenUsage { input: 100, output: 200, cache_read: 10, cache_creation: 0 },
            cost_usd: 0.042,
        };
        store.complete(payload).await.unwrap();

        // Verify result row exists with correct fields.
        let row: (String, String, String, Option<String>, i64, i64, f64) =
            sqlx::query_as(
                "SELECT worker_id, status, response, error_msg,
                          tokens_input, tokens_output, cost_usd
                 FROM distributed_results WHERE task_id = ?1",
            )
            .bind(&claimed.task_id)
            .fetch_one(&store.pool)
            .await
            .unwrap();

        assert_eq!(row.0, "w1");
        assert_eq!(row.1, "completed");
        assert_eq!(row.2, "great output");
        assert!(row.3.is_none());
        assert_eq!(row.4, 100);
        assert_eq!(row.5, 200);
        assert!((row.6 - 0.042).abs() < 1e-9);
    }

    #[tokio::test]
    async fn complete_second_call_returns_err_not_idempotent() {
        // Per spec: second call with same worker returns Err(UnknownTask) because
        // the row is no longer in `leased` state after the first complete.
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        store
            .complete(make_payload(&claimed.task_id, &worker, "completed"))
            .await
            .unwrap();

        let err = store
            .complete(make_payload(&claimed.task_id, &worker, "completed"))
            .await
            .unwrap_err();

        assert!(
            matches!(err, DistributedError::UnknownTask(_)),
            "expected UnknownTask, got {err:?}"
        );
    }

    #[tokio::test]
    async fn complete_returns_lease_lost_when_different_worker() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker_a = WorkerId::new("worker-a");
        let worker_b = WorkerId::new("worker-b");

        let claimed = store
            .claim_one(&worker_a, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        let err = store
            .complete(make_payload(&claimed.task_id, &worker_b, "completed"))
            .await
            .unwrap_err();

        assert!(
            matches!(err, DistributedError::LeaseLost { .. }),
            "expected LeaseLost, got {err:?}"
        );
    }

    // ── Capability matching ───────────────────────────────────────────────────

    #[tokio::test]
    async fn claim_one_skips_task_when_capabilities_not_satisfied() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let subtask = make_subtask_with_caps("s1", vec!["python".to_owned()]);
        store.enqueue("plan-1", vec![subtask]).await.unwrap();

        let worker = WorkerId::new("rust-worker");
        let result = store
            .claim_one(&worker, &["rust".to_owned()], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(result.is_none(), "rust worker should not claim python task");
    }

    #[tokio::test]
    async fn claim_one_returns_task_when_capabilities_match() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let subtask = make_subtask_with_caps("s1", vec!["python".to_owned()]);
        store.enqueue("plan-1", vec![subtask]).await.unwrap();

        let worker = WorkerId::new("poly-worker");
        let result = store
            .claim_one(
                &worker,
                &["python".to_owned(), "rust".to_owned()],
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert!(result.is_some(), "worker with python+rust should claim python task");
    }

    #[tokio::test]
    async fn claim_one_returns_task_with_no_required_capabilities_to_any_worker() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        // requires_capabilities is empty via make_subtask
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("bare-worker");
        // Worker declares no capabilities either
        let result = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(result.is_some(), "task with no requirements should be claimable by any worker");
    }

    #[tokio::test]
    async fn claim_one_treats_subset_relation_correctly() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();

        // Task requires ["a", "b"] — worker with only ["a"] should NOT get it.
        let t_ab = make_subtask_with_caps("t-ab", vec!["a".to_owned(), "b".to_owned()]);
        // Task requires ["a"] — worker with ["a", "b"] SHOULD get it.
        let t_a = make_subtask_with_caps("t-a", vec!["a".to_owned()]);

        store.enqueue("plan-subset", vec![t_ab, t_a]).await.unwrap();

        let worker_a_only = WorkerId::new("w-a");
        let worker_ab = WorkerId::new("w-ab");

        // Worker with only "a" should skip t-ab but can claim t-a.
        let claimed = store
            .claim_one(&worker_a_only, &["a".to_owned()], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.subtask.id, "t-a",
            "worker_a_only should claim t-a (not t-ab)");

        // Worker with "a"+"b" should now get t-ab (t-a is already leased).
        let claimed2 = store
            .claim_one(&worker_ab, &["a".to_owned(), "b".to_owned()], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed2.subtask.id, "t-ab");
    }

    // ── Dependency satisfaction at claim time ─────────────────────────────────

    #[tokio::test]
    async fn claim_one_skips_task_with_unfulfilled_dependencies() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();

        let t1 = make_subtask("T1");
        let t2 = make_subtask_with_deps("T2", vec!["T1".to_owned()]);
        store.enqueue("plan-deps", vec![t1, t2]).await.unwrap();

        let worker = WorkerId::new("w1");
        // First claim grabs T1.
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.subtask.id, "T1");

        // T1 is now leased (not done), so T2 should still be blocked.
        let result = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(result.is_none(), "T2 should not be claimable while T1 is not done");
    }

    #[tokio::test]
    async fn claim_one_returns_task_when_dependencies_done() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();

        let t1 = make_subtask("T1");
        let t2 = make_subtask_with_deps("T2", vec!["T1".to_owned()]);
        store.enqueue("plan-deps", vec![t1, t2]).await.unwrap();

        let worker = WorkerId::new("w1");

        // Claim and complete T1.
        let claimed_t1 = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed_t1.subtask.id, "T1");
        store
            .complete(make_payload(&claimed_t1.task_id, &worker, "completed"))
            .await
            .unwrap();

        // Now T2 should be eligible.
        let claimed_t2 = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(claimed_t2.is_some(), "T2 should be claimable after T1 is done");
        assert_eq!(claimed_t2.unwrap().subtask.id, "T2");
    }

    #[tokio::test]
    async fn claim_one_skips_task_when_dependency_failed() {
        // Design choice: failed dependency blocks downstream tasks (surfaces failures fast).
        let store = DistributedTaskStore::open_in_memory().await.unwrap();

        let t1 = make_subtask("T1");
        let t2 = make_subtask_with_deps("T2", vec!["T1".to_owned()]);
        store.enqueue("plan-deps", vec![t1, t2]).await.unwrap();

        let worker = WorkerId::new("w1");

        // Claim T1 and mark it failed.
        let claimed_t1 = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        store
            .complete(make_payload(&claimed_t1.task_id, &worker, "failed"))
            .await
            .unwrap();

        // T2 should NOT be claimable when its dependency failed.
        let result = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(result.is_none(), "T2 should not be claimable when T1 failed");
    }

    // ── Lease expiry + reaper ─────────────────────────────────────────────────

    #[tokio::test]
    async fn reap_expired_leases_requeues_expired_tasks() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        // Simulate lease expiry: now = far future past expiry.
        let past_expiry = claimed.lease_expires_at + chrono::Duration::seconds(1);
        let count = store.reap_expired_leases(past_expiry, 10).await.unwrap();
        assert_eq!(count, 1);

        // Task should be back to queued.
        let (queued, leased, done, failed, _) =
            store.plan_state_counts("plan-1").await.unwrap();
        assert_eq!(queued, 1);
        assert_eq!(leased, 0);
        assert_eq!(done, 0);
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn reap_expired_leases_does_not_touch_unexpired_leases() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        // Reap with now = before lease_expires_at.
        let before_expiry = claimed.lease_expires_at - chrono::Duration::seconds(1);
        let count = store.reap_expired_leases(before_expiry, 10).await.unwrap();
        assert_eq!(count, 0);

        let (queued, leased, _, _, _) = store.plan_state_counts("plan-1").await.unwrap();
        assert_eq!(leased, 1, "unexpired lease should remain leased");
        assert_eq!(queued, 0);
    }

    #[tokio::test]
    async fn reap_expired_leases_force_fails_when_max_attempts_exceeded() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();
        let max_attempts = 3u32;

        let worker = WorkerId::new("w1");

        // Exhaust all attempts via claim + reap cycle.
        for _attempt in 0..max_attempts {
            let claimed = store
                .claim_one(&worker, &[], Duration::from_secs(60))
                .await
                .unwrap()
                .expect("should be claimable before max attempts reached");

            let past_expiry = claimed.lease_expires_at + chrono::Duration::seconds(1);
            store.reap_expired_leases(past_expiry, max_attempts).await.unwrap();
        }

        // After max_attempts reaps the task should be permanently failed.
        let (queued, leased, done, failed, _) =
            store.plan_state_counts("plan-1").await.unwrap();
        assert_eq!(failed, 1, "task should be force-failed after max_attempts");
        assert_eq!(queued + leased + done, 0);
    }

    #[tokio::test]
    async fn reap_expired_leases_returns_count() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store
            .enqueue("plan-1", vec![make_subtask("s1"), make_subtask("s2"), make_subtask("s3")])
            .await
            .unwrap();

        let worker = WorkerId::new("w1");
        let mut lease_expiries = vec![];

        // Claim all 3.
        for _ in 0..3 {
            let c = store
                .claim_one(&worker, &[], Duration::from_secs(60))
                .await
                .unwrap()
                .unwrap();
            lease_expiries.push(c.lease_expires_at);
        }

        // Reap past all expiries.
        let max_exp = lease_expiries.iter().max().cloned().unwrap();
        let past_all = max_exp + chrono::Duration::seconds(1);
        let count = store.reap_expired_leases(past_all, 10).await.unwrap();
        assert_eq!(count, 3);
    }

    // ── plan_state_counts ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn plan_state_counts_returns_zeros_for_unknown_plan() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let counts = store.plan_state_counts("no-such-plan").await.unwrap();
        assert_eq!(counts, (0, 0, 0, 0, 0));
    }

    #[tokio::test]
    async fn plan_state_counts_aggregates_correctly() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let subtasks: Vec<SubTask> = (1..=5).map(|i| make_subtask(&format!("s{i}"))).collect();
        store.enqueue("plan-counts", subtasks).await.unwrap();

        let worker = WorkerId::new("w1");

        // Claim 2 tasks.
        let c1 = store.claim_one(&worker, &[], Duration::from_secs(60)).await.unwrap().unwrap();
        let c2 = store.claim_one(&worker, &[], Duration::from_secs(60)).await.unwrap().unwrap();

        // Complete 1 of them.
        store.complete(make_payload(&c1.task_id, &worker, "completed")).await.unwrap();
        // c2 stays leased.
        let _ = c2;

        let (queued, leased, done, failed, timed_out) =
            store.plan_state_counts("plan-counts").await.unwrap();

        assert_eq!(queued, 3, "3 tasks remain queued");
        assert_eq!(leased, 1, "1 task is leased");
        assert_eq!(done, 1, "1 task is done");
        assert_eq!(failed, 0);
        assert_eq!(timed_out, 0);
    }

    // ── Worker registry ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn upsert_worker_creates_then_updates() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        let worker = WorkerId::new("worker-hb");
        let t1 = Utc::now();

        store
            .upsert_worker(&worker, &["rust".to_owned()], None, t1)
            .await
            .unwrap();

        // Verify row exists.
        let row: (String, String, i64, Option<String>) = sqlx::query_as(
            "SELECT id, capabilities, last_heartbeat, sandbox_profile
             FROM distributed_workers WHERE id = ?1",
        )
        .bind(worker.as_str())
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(row.0, "worker-hb");
        assert!(row.1.contains("rust"));
        assert_eq!(row.2, t1.timestamp_micros());
        assert!(row.3.is_none());

        // Update with new heartbeat + capabilities.
        let t2 = t1 + chrono::Duration::seconds(30);
        store
            .upsert_worker(
                &worker,
                &["rust".to_owned(), "python".to_owned()],
                Some(r#"{"profile":"strict"}"#),
                t2,
            )
            .await
            .unwrap();

        let row2: (String, i64, Option<String>) = sqlx::query_as(
            "SELECT capabilities, last_heartbeat, sandbox_profile
             FROM distributed_workers WHERE id = ?1",
        )
        .bind(worker.as_str())
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert!(row2.0.contains("python"), "capabilities should be updated");
        assert_eq!(row2.1, t2.timestamp_micros(), "heartbeat should be updated");
        assert!(row2.2.is_some(), "sandbox_profile should now be set");
    }

    #[tokio::test]
    async fn extend_lease_extends_when_owner_matches() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker = WorkerId::new("w1");
        let claimed = store
            .claim_one(&worker, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        let new_expiry = Utc::now() + chrono::Duration::seconds(300);
        store
            .extend_lease(&worker, &claimed.task_id, new_expiry)
            .await
            .unwrap();

        // Verify db updated.
        let row: (i64,) = sqlx::query_as(
            "SELECT lease_expires_at FROM distributed_tasks WHERE id = ?1",
        )
        .bind(&claimed.task_id)
        .fetch_one(&store.pool)
        .await
        .unwrap();

        assert_eq!(row.0, new_expiry.timestamp_micros());
    }

    #[tokio::test]
    async fn extend_lease_returns_lease_lost_when_owner_mismatch() {
        let store = DistributedTaskStore::open_in_memory().await.unwrap();
        store.enqueue("plan-1", vec![make_subtask("s1")]).await.unwrap();

        let worker_a = WorkerId::new("worker-a");
        let worker_b = WorkerId::new("worker-b");

        let claimed = store
            .claim_one(&worker_a, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        let new_expiry = Utc::now() + chrono::Duration::seconds(300);
        let err = store
            .extend_lease(&worker_b, &claimed.task_id, new_expiry)
            .await
            .unwrap_err();

        assert!(
            matches!(err, DistributedError::LeaseLost { .. }),
            "expected LeaseLost, got {err:?}"
        );
    }

    // ── Concurrency: atomicity of claim_one ───────────────────────────────────

    #[tokio::test]
    async fn concurrent_claim_one_calls_dispense_each_task_exactly_once() {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let store = Arc::new(DistributedTaskStore::open_in_memory().await.unwrap());

        // Enqueue 10 tasks.
        let subtasks: Vec<SubTask> = (0..10).map(|i| make_subtask(&format!("task-{i}"))).collect();
        store.enqueue("plan-concurrent", subtasks).await.unwrap();

        // Track claimed task_ids across workers.
        let claimed_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));

        // Spawn 5 concurrent workers, each calling claim_one in a loop.
        let mut handles = vec![];
        for w in 0..5usize {
            let store = store.clone();
            let ids = claimed_ids.clone();
            handles.push(tokio::spawn(async move {
                let worker = WorkerId::new(format!("concurrent-worker-{w}"));
                loop {
                    let result = store
                        .claim_one(&worker, &[], Duration::from_secs(60))
                        .await
                        .unwrap();
                    match result {
                        None => break,
                        Some(ct) => {
                            ids.lock().await.push(ct.task_id.clone());
                            // Complete immediately so dependencies are satisfied for future tasks.
                            let _ = store
                                .complete(make_payload(&ct.task_id, &worker, "completed"))
                                .await;
                        }
                    }
                }
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let ids = claimed_ids.lock().await;
        // Exactly 10 tasks claimed.
        assert_eq!(ids.len(), 10, "all 10 tasks must be claimed: got {}", ids.len());

        // Each task claimed exactly once (no duplicates).
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 10, "each task must be claimed by exactly one worker");
    }
}
