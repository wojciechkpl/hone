//! Crash-recovery checkpointing for the orchestrator.
//!
//! After each worker completes, the orchestrator saves its current state to
//! a SQLite database.  If the process is killed, `Orchestrator::resume` can
//! reload the checkpoint and continue from where it left off.
//!
//! # Schema
//!
//! ```sql
//! CREATE TABLE orchestration_checkpoints (
//!     run_id            TEXT PRIMARY KEY,
//!     goal              TEXT NOT NULL,
//!     plan_json         TEXT NOT NULL,
//!     completed_json    TEXT NOT NULL,
//!     pending_ids_json  TEXT NOT NULL,
//!     total_cost_usd    REAL NOT NULL,
//!     created_at        INTEGER NOT NULL,
//!     updated_at        INTEGER NOT NULL,
//!     finished          INTEGER NOT NULL DEFAULT 0
//! );
//! ```

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

use crate::task_plan::{TaskPlan, WorkerResult, WorkerStatus};
use hone_proto::TokenUsage;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during checkpoint operations.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// The checkpoint database could not be opened or created.
    #[error("failed to open checkpoint database at {path}: {source}")]
    Open {
        path: std::path::PathBuf,
        #[source]
        source: sqlx::Error,
    },
    /// A required DDL or DML statement failed.
    #[error("checkpoint SQL error: {context}: {source}")]
    Sql {
        context: String,
        #[source]
        source: sqlx::Error,
    },
    /// The checkpoint store was used before `bind_run` was called.
    #[error("checkpoint store is not bound to a run")]
    NotBound,
    /// The stored JSON could not be deserialised — the record is corrupted.
    #[error("checkpoint data is corrupted ({context}): {source}")]
    Corrupted {
        context: String,
        #[source]
        source: serde_json::Error,
    },
    /// The requested checkpoint run was not found.
    #[error("checkpoint run '{run_id}' not found")]
    NotFound { run_id: String },
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of an orchestration run.
///
/// `completed_workers` holds the results already finished before the run
/// was interrupted.  They are serialised via a DTO to avoid requiring serde
/// on `WorkerResult` itself.
#[derive(Debug, Clone)]
pub struct OrchestrationCheckpoint {
    pub run_id: String,
    pub goal: String,
    pub plan: TaskPlan,
    pub completed_workers: Vec<WorkerResult>,
    pub pending_task_ids: Vec<String>,
    pub total_cost_usd: f64,
    pub created_at: u64,
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// Serialisable DTO for WorkerResult
// ---------------------------------------------------------------------------
//
// `WorkerResult` does not implement `Serialize`/`Deserialize` (hone-proto
// types are intentionally kept serde-free to avoid coupling).  We map through
// a thin DTO that lives only in this module.

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResultDto {
    task_id: String,
    agent: String,
    status: WorkerStatusDto,
    response: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    #[serde(default)]
    cache_creation_tokens: u32,
    cost_usd: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "msg")]
enum WorkerStatusDto {
    Completed,
    Failed(String),
    TimedOut(String),
}

impl From<&WorkerResult> for WorkerResultDto {
    fn from(r: &WorkerResult) -> Self {
        WorkerResultDto {
            task_id: r.task_id.clone(),
            agent: r.agent.clone(),
            status: match &r.status {
                WorkerStatus::Completed => WorkerStatusDto::Completed,
                WorkerStatus::Failed(m) => WorkerStatusDto::Failed(m.clone()),
                WorkerStatus::TimedOut(m) => WorkerStatusDto::TimedOut(m.clone()),
            },
            response: r.response.clone(),
            input_tokens: r.usage.input,
            output_tokens: r.usage.output,
            cache_read_tokens: r.usage.cache_read,
            cache_creation_tokens: r.usage.cache_creation,
            cost_usd: r.cost_usd,
        }
    }
}

impl From<WorkerResultDto> for WorkerResult {
    fn from(d: WorkerResultDto) -> Self {
        WorkerResult {
            task_id: d.task_id,
            agent: d.agent,
            status: match d.status {
                WorkerStatusDto::Completed => WorkerStatus::Completed,
                WorkerStatusDto::Failed(m) => WorkerStatus::Failed(m),
                WorkerStatusDto::TimedOut(m) => WorkerStatus::TimedOut(m),
            },
            response: d.response,
            usage: TokenUsage {
                input: d.input_tokens,
                output: d.output_tokens,
                cache_read: d.cache_read_tokens,
                cache_creation: d.cache_creation_tokens,
            },
            cost_usd: d.cost_usd,
        }
    }
}

// ---------------------------------------------------------------------------
// CheckpointStore
// ---------------------------------------------------------------------------

/// SQLite-backed store for orchestration checkpoints.
pub struct CheckpointStore {
    pool: SqlitePool,
    /// Identifier for the current run.  Set when the store is bound to an
    /// orchestration via `bind_run`.
    run_id: Option<String>,
}

impl CheckpointStore {
    /// Open (or create) the checkpoint database at `path`.
    pub async fn open(path: &Path) -> Result<Self, CheckpointError> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(opts)
            .await
            .map_err(|e| CheckpointError::Open {
                path: path.to_path_buf(),
                source: e,
            })?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS orchestration_checkpoints (
                run_id           TEXT PRIMARY KEY,
                goal             TEXT NOT NULL,
                plan_json        TEXT NOT NULL,
                completed_json   TEXT NOT NULL,
                pending_ids_json TEXT NOT NULL,
                total_cost_usd   REAL NOT NULL,
                created_at       INTEGER NOT NULL,
                updated_at       INTEGER NOT NULL,
                finished         INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to create orchestration_checkpoints table".to_owned(),
            source: e,
        })?;

        Ok(Self { pool, run_id: None })
    }

    /// Bind this store to a specific run.  Must be called before
    /// `save_progress`.  Inserts the initial checkpoint row.
    pub async fn bind_run(
        &mut self,
        run_id: &str,
        goal: &str,
        plan: &TaskPlan,
    ) -> Result<(), CheckpointError> {
        self.run_id = Some(run_id.to_owned());

        let plan_json = serde_json::to_string(plan).map_err(|e| CheckpointError::Corrupted {
            context: "failed to serialise plan".to_owned(),
            source: e,
        })?;
        let pending_ids: Vec<String> = plan.subtasks.iter().map(|t| t.id.clone()).collect();
        let pending_json =
            serde_json::to_string(&pending_ids).map_err(|e| CheckpointError::Corrupted {
                context: "failed to serialise pending ids".to_owned(),
                source: e,
            })?;
        let now = unix_now();

        sqlx::query(
            "INSERT OR IGNORE INTO orchestration_checkpoints
             (run_id, goal, plan_json, completed_json, pending_ids_json,
              total_cost_usd, created_at, updated_at, finished)
             VALUES (?, ?, ?, '[]', ?, 0.0, ?, ?, 0)",
        )
        .bind(run_id)
        .bind(goal)
        .bind(&plan_json)
        .bind(&pending_json)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to insert initial checkpoint".to_owned(),
            source: e,
        })?;

        Ok(())
    }

    /// Persist progress after a worker completes.
    ///
    /// Call this from `execute_plan` after each `join_next` result.
    pub async fn save_progress(
        &self,
        completed_workers: &[WorkerResult],
        pending_task_ids: &[String],
        total_cost_usd: f64,
    ) -> Result<(), CheckpointError> {
        let run_id = self.run_id.as_deref().ok_or(CheckpointError::NotBound)?;

        let dtos: Vec<WorkerResultDto> = completed_workers.iter().map(Into::into).collect();
        let completed_json =
            serde_json::to_string(&dtos).map_err(|e| CheckpointError::Corrupted {
                context: "failed to serialise completed workers".to_owned(),
                source: e,
            })?;
        let pending_json =
            serde_json::to_string(pending_task_ids).map_err(|e| CheckpointError::Corrupted {
                context: "failed to serialise pending ids".to_owned(),
                source: e,
            })?;

        sqlx::query(
            "UPDATE orchestration_checkpoints
             SET completed_json = ?, pending_ids_json = ?, total_cost_usd = ?, updated_at = ?
             WHERE run_id = ?",
        )
        .bind(&completed_json)
        .bind(&pending_json)
        .bind(total_cost_usd)
        .bind(unix_now())
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to update checkpoint".to_owned(),
            source: e,
        })?;

        Ok(())
    }

    /// Save a complete `OrchestrationCheckpoint` (used for explicit saves and tests).
    pub async fn save(&self, checkpoint: &OrchestrationCheckpoint) -> Result<(), CheckpointError> {
        let plan_json =
            serde_json::to_string(&checkpoint.plan).map_err(|e| CheckpointError::Corrupted {
                context: "failed to serialise plan".to_owned(),
                source: e,
            })?;
        let dtos: Vec<WorkerResultDto> = checkpoint
            .completed_workers
            .iter()
            .map(Into::into)
            .collect();
        let completed_json =
            serde_json::to_string(&dtos).map_err(|e| CheckpointError::Corrupted {
                context: "failed to serialise completed workers".to_owned(),
                source: e,
            })?;
        let pending_json = serde_json::to_string(&checkpoint.pending_task_ids).map_err(|e| {
            CheckpointError::Corrupted {
                context: "failed to serialise pending ids".to_owned(),
                source: e,
            }
        })?;

        sqlx::query(
            "INSERT OR REPLACE INTO orchestration_checkpoints
             (run_id, goal, plan_json, completed_json, pending_ids_json,
              total_cost_usd, created_at, updated_at, finished)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(&checkpoint.run_id)
        .bind(&checkpoint.goal)
        .bind(&plan_json)
        .bind(&completed_json)
        .bind(&pending_json)
        .bind(checkpoint.total_cost_usd)
        .bind(checkpoint.created_at as i64)
        .bind(checkpoint.updated_at as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to save checkpoint".to_owned(),
            source: e,
        })?;

        Ok(())
    }

    /// Load a checkpoint by `run_id`.  Returns `None` when not found or
    /// when the run has already been marked as finished.
    pub async fn load(
        &self,
        run_id: &str,
    ) -> Result<Option<OrchestrationCheckpoint>, CheckpointError> {
        let row: Option<(String, String, String, String, String, f64, i64, i64)> = sqlx::query_as(
            "SELECT goal, plan_json, completed_json, pending_ids_json, run_id,
                        total_cost_usd, created_at, updated_at
                 FROM orchestration_checkpoints
                 WHERE run_id = ? AND finished = 0",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to query checkpoint".to_owned(),
            source: e,
        })?;

        let Some((goal, plan_json, completed_json, pending_json, rid, cost, created, updated)) =
            row
        else {
            return Ok(None);
        };

        let plan: TaskPlan =
            serde_json::from_str(&plan_json).map_err(|e| CheckpointError::Corrupted {
                context: "failed to deserialise plan".to_owned(),
                source: e,
            })?;
        let dtos: Vec<WorkerResultDto> =
            serde_json::from_str(&completed_json).map_err(|e| CheckpointError::Corrupted {
                context: "failed to deserialise workers".to_owned(),
                source: e,
            })?;
        let completed_workers: Vec<WorkerResult> = dtos.into_iter().map(Into::into).collect();
        let pending_task_ids: Vec<String> =
            serde_json::from_str(&pending_json).map_err(|e| CheckpointError::Corrupted {
                context: "failed to deserialise pending ids".to_owned(),
                source: e,
            })?;

        Ok(Some(OrchestrationCheckpoint {
            run_id: rid,
            goal,
            plan,
            completed_workers,
            pending_task_ids,
            total_cost_usd: cost,
            created_at: created as u64,
            updated_at: updated as u64,
        }))
    }

    /// Return the `run_id`s of all incomplete (not yet finished) checkpoints.
    pub async fn list_incomplete(&self) -> Result<Vec<String>, CheckpointError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT run_id FROM orchestration_checkpoints WHERE finished = 0")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| CheckpointError::Sql {
                    context: "failed to list incomplete checkpoints".to_owned(),
                    source: e,
                })?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Mark a run as successfully finished so it no longer appears in
    /// `list_incomplete`.
    pub async fn mark_finished(&self, run_id: &str) -> Result<(), CheckpointError> {
        sqlx::query(
            "UPDATE orchestration_checkpoints
             SET finished = 1, updated_at = ?
             WHERE run_id = ?",
        )
        .bind(unix_now())
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_err(|e| CheckpointError::Sql {
            context: "failed to mark checkpoint finished".to_owned(),
            source: e,
        })?;

        Ok(())
    }

    /// Delete a checkpoint row entirely.
    pub async fn delete(&self, run_id: &str) -> Result<(), CheckpointError> {
        sqlx::query("DELETE FROM orchestration_checkpoints WHERE run_id = ?")
            .bind(run_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Sql {
                context: "failed to delete checkpoint".to_owned(),
                source: e,
            })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_plan::SubTask;
    use tempfile::tempdir;

    fn make_plan(goal: &str) -> TaskPlan {
        TaskPlan {
            goal: goal.to_owned(),
            subtasks: vec![
                SubTask {
                    id: "t1".to_owned(),
                    description: "task one".to_owned(),
                    agent: "rust-expert".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                },
                SubTask {
                    id: "t2".to_owned(),
                    description: "task two".to_owned(),
                    agent: "debugger".to_owned(),
                    context: String::new(),
                    depends_on: vec!["t1".to_owned()],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                },
            ],
        }
    }

    fn make_worker_result(task_id: &str, status: WorkerStatus) -> WorkerResult {
        WorkerResult {
            task_id: task_id.to_owned(),
            agent: "rust-expert".to_owned(),
            status,
            response: "done".to_owned(),
            usage: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_creation: 0,
            },
            cost_usd: 0.001,
        }
    }

    async fn open_store() -> (CheckpointStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("checkpoints.db");
        let store = CheckpointStore::open(&path).await.expect("open");
        (store, dir)
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (store, _dir) = open_store().await;
        let plan = make_plan("implement feature X");

        let checkpoint = OrchestrationCheckpoint {
            run_id: "run-001".to_owned(),
            goal: "implement feature X".to_owned(),
            plan: plan.clone(),
            completed_workers: vec![make_worker_result("t1", WorkerStatus::Completed)],
            pending_task_ids: vec!["t2".to_owned()],
            total_cost_usd: 0.001,
            created_at: 1_000_000,
            updated_at: 1_000_001,
        };

        store.save(&checkpoint).await.expect("save");

        let loaded = store.load("run-001").await.expect("load").expect("some");
        assert_eq!(loaded.run_id, "run-001");
        assert_eq!(loaded.goal, "implement feature X");
        assert_eq!(loaded.completed_workers.len(), 1);
        assert_eq!(loaded.completed_workers[0].task_id, "t1");
        assert_eq!(loaded.pending_task_ids, vec!["t2"]);
        assert!((loaded.total_cost_usd - 0.001).abs() < 1e-9);
    }

    #[tokio::test]
    async fn load_nonexistent_returns_none() {
        let (store, _dir) = open_store().await;
        let result = store.load("does-not-exist").await.expect("load");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_incomplete_excludes_finished() {
        let (store, _dir) = open_store().await;
        let plan = make_plan("goal");

        for id in ["run-a", "run-b", "run-c"] {
            let cp = OrchestrationCheckpoint {
                run_id: id.to_owned(),
                goal: "goal".to_owned(),
                plan: plan.clone(),
                completed_workers: vec![],
                pending_task_ids: vec!["t1".to_owned()],
                total_cost_usd: 0.0,
                created_at: 1,
                updated_at: 1,
            };
            store.save(&cp).await.expect("save");
        }

        store.mark_finished("run-b").await.expect("mark");

        let incomplete = store.list_incomplete().await.expect("list");
        assert_eq!(incomplete.len(), 2);
        assert!(incomplete.contains(&"run-a".to_owned()));
        assert!(incomplete.contains(&"run-c".to_owned()));
        assert!(!incomplete.contains(&"run-b".to_owned()));
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let (store, _dir) = open_store().await;
        let plan = make_plan("goal");
        let cp = OrchestrationCheckpoint {
            run_id: "run-del".to_owned(),
            goal: "goal".to_owned(),
            plan,
            completed_workers: vec![],
            pending_task_ids: vec![],
            total_cost_usd: 0.0,
            created_at: 1,
            updated_at: 1,
        };
        store.save(&cp).await.expect("save");
        store.delete("run-del").await.expect("delete");
        assert!(store.load("run-del").await.expect("load").is_none());
    }

    #[tokio::test]
    async fn timed_out_status_roundtrips() {
        let (store, _dir) = open_store().await;
        let plan = make_plan("goal");
        let cp = OrchestrationCheckpoint {
            run_id: "run-timeout".to_owned(),
            goal: "goal".to_owned(),
            plan,
            completed_workers: vec![make_worker_result(
                "t1",
                WorkerStatus::TimedOut("exceeded 300s".to_owned()),
            )],
            pending_task_ids: vec!["t2".to_owned()],
            total_cost_usd: 0.0,
            created_at: 1,
            updated_at: 1,
        };
        store.save(&cp).await.expect("save");
        let loaded = store
            .load("run-timeout")
            .await
            .expect("load")
            .expect("some");
        assert!(
            matches!(
                &loaded.completed_workers[0].status,
                WorkerStatus::TimedOut(m) if m == "exceeded 300s"
            ),
            "TimedOut status must survive a save/load roundtrip"
        );
    }

    #[tokio::test]
    async fn save_progress_updates_row() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("checkpoints.db");
        let mut store = CheckpointStore::open(&path).await.expect("open");
        let plan = make_plan("goal");

        store
            .bind_run("run-prog", "goal", &plan)
            .await
            .expect("bind");

        let w1 = make_worker_result("t1", WorkerStatus::Completed);
        store
            .save_progress(&[w1], &["t2".to_owned()], 0.001)
            .await
            .expect("save_progress");

        let loaded = store.load("run-prog").await.expect("load").expect("some");
        assert_eq!(loaded.completed_workers.len(), 1);
        assert_eq!(loaded.pending_task_ids, vec!["t2"]);
        assert!((loaded.total_cost_usd - 0.001).abs() < 1e-9);
    }

    #[tokio::test]
    async fn cache_creation_tokens_round_trip() {
        let (mut store, _dir) = open_store().await;
        let plan = make_plan("goal");
        store
            .bind_run("run-cache", "goal", &plan)
            .await
            .expect("bind");

        let worker = WorkerResult {
            task_id: "t1".to_owned(),
            agent: "rust-expert".to_owned(),
            status: WorkerStatus::Completed,
            response: "done".to_owned(),
            usage: TokenUsage {
                input: 100,
                output: 50,
                cache_read: 2_000,
                cache_creation: 5_000,
            },
            cost_usd: 0.01,
        };
        store
            .save_progress(&[worker], &["t2".to_owned()], 0.01)
            .await
            .expect("save_progress");

        let loaded = store.load("run-cache").await.expect("load").expect("some");
        let restored = &loaded.completed_workers[0];
        assert_eq!(
            restored.usage.cache_creation, 5_000,
            "cache_creation must survive roundtrip"
        );
        assert_eq!(restored.usage.cache_read, 2_000);
    }
}
