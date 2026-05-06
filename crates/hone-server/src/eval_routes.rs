//! `GET /evals` and `GET /evals/{id}` — read-only viewer over the
//! eval-history sqlite store the `hone eval` CLI populates.
//!
//! Both endpoints return 503 when no store is configured. The list
//! endpoint paginates via `?limit=` (default 50, max 200) and accepts an
//! optional `?tag=` filter. The detail endpoint returns the full
//! `EvalRunRecord` including every per-task `TaskOutcome`.

use axum::Json;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use hone_eval::{EvalRunRecord, RunSummary};
use serde::Deserialize;

use crate::state::AppState;

/// Default page size for `GET /evals` when the client does not specify
/// `limit`. Generous enough to show a session's worth of runs in one
/// fetch without flooding the GUI.
const DEFAULT_LIMIT: usize = 50;
/// Hard ceiling enforced regardless of `?limit=`.
const MAX_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional tag filter; matches `EvalRunRecord::tag` exactly.
    #[serde(default)]
    pub tag: Option<String>,
}

/// `GET /evals?limit=N&tag=...`
pub async fn list_evals_handler(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<RunSummary>>, (StatusCode, String)> {
    let store = state
        .eval_history
        .as_ref()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "eval history not configured".to_string(),
        ))?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let summaries = store
        .list(q.tag.as_deref(), limit)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("list failed: {e}")))?;
    Ok(Json(summaries))
}

/// `GET /evals/{id}` — full record with per-task outcomes.
pub async fn eval_detail_handler(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<EvalRunRecord>, (StatusCode, String)> {
    let store = state
        .eval_history
        .as_ref()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "eval history not configured".to_string(),
        ))?;
    if id.is_empty() || id.contains(['/', '\\', '\0']) {
        return Err((StatusCode::BAD_REQUEST, format!("invalid run id: {id:?}")));
    }
    let record = store
        .get(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("get failed: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, format!("no run named '{id}'")))?;
    Ok(Json(record))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::Utc;
    use hone_eval::{EvalRunStore, TaskOutcome, TaskStatus};
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn empty_store() -> Arc<EvalRunStore> {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("eval.db");
        let store = EvalRunStore::open(&path).await.expect("open");
        // Tempdir drops at end of test; keep the path alive by leaking the
        // dir intentionally — this is a unit test helper.
        std::mem::forget(tmp);
        Arc::new(store)
    }

    fn fake_outcomes(passed: usize, total: usize) -> Vec<TaskOutcome> {
        (0..total)
            .map(|i| TaskOutcome {
                id: format!("t{i}"),
                status: if i < passed {
                    TaskStatus::Pass
                } else {
                    TaskStatus::Fail
                },
                duration_ms: 100,
                tokens_input: 10,
                tokens_output: 20,
                cost_usd: 0.001,
                success_check_stdout: String::new(),
                success_check_stderr: String::new(),
                success_check_exit_code: Some(0),
            })
            .collect()
    }

    async fn record(
        store: &EvalRunStore,
        model: &str,
        passed: usize,
        total: usize,
    ) -> EvalRunRecord {
        store
            .record(None, model.to_string(), "hash".into(), None, fake_outcomes(passed, total))
            .await
            .expect("record")
    }
    // chrono::Utc kept imported for documentation; suppress warning when
    // the import is unused after refactor.
    #[allow(dead_code)]
    fn _suppress_unused_utc_import() -> chrono::DateTime<Utc> {
        Utc::now()
    }

    #[tokio::test]
    async fn list_handler_returns_503_when_no_store() {
        let state = AppState::new(None);
        let q = Query(ListQuery { limit: None, tag: None });
        let resp = list_evals_handler(axum::extract::State(state), q).await;
        match resp {
            Ok(_) => panic!("expected 503"),
            Err((code, _)) => assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE),
        }
    }

    #[tokio::test]
    async fn list_handler_returns_summaries_when_store_attached() {
        let store = empty_store().await;
        let r1 = record(&store, "claude", 3, 5).await;
        let state = AppState::new(None).with_eval_history(store);
        let q = Query(ListQuery { limit: None, tag: None });
        let json = list_evals_handler(axum::extract::State(state), q)
            .await
            .expect("ok");
        let arr = json.0;
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].id, r1.id);
        assert_eq!(arr[0].passed, 3);
        assert_eq!(arr[0].total, 5);
    }

    #[tokio::test]
    async fn detail_handler_returns_full_record() {
        let store = empty_store().await;
        let r2 = record(&store, "gpt", 1, 2).await;
        let state = AppState::new(None).with_eval_history(store);
        let json = eval_detail_handler(axum::extract::State(state), AxumPath(r2.id.clone()))
            .await
            .expect("ok");
        assert_eq!(json.0.id, r2.id);
        assert_eq!(json.0.outcomes.len(), 2);
    }

    #[tokio::test]
    async fn detail_handler_returns_404_for_unknown_id() {
        let store = empty_store().await;
        let state = AppState::new(None).with_eval_history(store);
        let resp = eval_detail_handler(axum::extract::State(state), AxumPath("nope".into())).await;
        match resp {
            Ok(_) => panic!("expected 404"),
            Err((code, _)) => assert_eq!(code, StatusCode::NOT_FOUND),
        }
    }

    #[tokio::test]
    async fn detail_handler_rejects_path_traversal() {
        let store = empty_store().await;
        let state = AppState::new(None).with_eval_history(store);
        let resp = eval_detail_handler(
            axum::extract::State(state),
            AxumPath("../etc/passwd".into()),
        )
        .await;
        match resp {
            Ok(_) => panic!("must reject path traversal"),
            Err((code, _)) => assert_eq!(code, StatusCode::BAD_REQUEST),
        }
    }
}
