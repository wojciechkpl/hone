use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::distributed::{DistributedTaskStore, WorkerId};
use crate::state::AppState;
use hone_core::task_plan::SubTask;

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EnqueueRequest {
    pub subtasks: Vec<SubTask>,
}

#[derive(Debug, Serialize)]
pub struct EnqueueResponse {
    pub task_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanStatusResponse {
    pub queued: usize,
    pub leased: usize,
    pub done: usize,
    pub failed: usize,
    pub timed_out: usize,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub capabilities: Vec<String>,
    pub sandbox_profile: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    pub capabilities: Vec<String>,
    /// Optional override for lease duration in seconds (default 60).
    pub lease_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ExtendLeaseRequest {
    pub extend_secs: u64,
}

/// Body accepted by `POST /distributed/workers/:worker_id/results/:task_id`.
/// `worker_id` and `task_id` are taken from the URL path; the body carries
/// the completion details only.
#[derive(Debug, Deserialize)]
pub struct ResultsRequest {
    pub status_str: String,
    pub response: String,
    pub error_msg: Option<String>,
    pub usage: hone_proto::TokenUsage,
    pub cost_usd: f64,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the `/distributed/*` sub-router.
/// Mount this inside the auth-protected router so all endpoints require a bearer token.
pub fn distributed_router() -> Router<AppState> {
    Router::new()
        .route(
            "/distributed/plans/{plan_id}/enqueue",
            post(enqueue_handler),
        )
        .route(
            "/distributed/plans/{plan_id}/status",
            get(plan_status_handler),
        )
        .route(
            "/distributed/plans/{plan_id}/results",
            get(plan_results_handler),
        )
        .route(
            "/distributed/workers/{worker_id}/heartbeat",
            post(heartbeat_handler),
        )
        .route(
            "/distributed/workers/{worker_id}/claim",
            post(claim_handler),
        )
        .route(
            "/distributed/workers/{worker_id}/results/{task_id}",
            post(results_handler),
        )
        .route(
            "/distributed/workers/{worker_id}/extend-lease/{task_id}",
            post(extend_lease_handler),
        )
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extracts the distributed store from AppState or returns 503.
fn require_store(state: &AppState) -> Option<Arc<DistributedTaskStore>> {
    state.distributed_store.clone()
}

fn store_unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "distributed orchestrator not configured" })),
    )
        .into_response()
}

fn lease_lost_response() -> axum::response::Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "error": "lease lost" })),
    )
        .into_response()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn enqueue_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
    Json(req): Json<EnqueueRequest>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    match store.enqueue(&plan_id, req.subtasks).await {
        Ok(ids) => (StatusCode::OK, Json(EnqueueResponse { task_ids: ids })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn plan_status_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    match store.plan_state_counts(&plan_id).await {
        Ok((queued, leased, done, failed, timed_out)) => (
            StatusCode::OK,
            Json(PlanStatusResponse {
                queued,
                leased,
                done,
                failed,
                timed_out,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn heartbeat_handler(
    State(state): State<AppState>,
    Path(worker_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    let wid = WorkerId::new(worker_id);
    let sandbox = req.sandbox_profile.as_deref();
    match store
        .upsert_worker(&wid, &req.capabilities, sandbox, chrono::Utc::now())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn claim_handler(
    State(state): State<AppState>,
    Path(worker_id): Path<String>,
    Json(req): Json<ClaimRequest>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    let wid = WorkerId::new(worker_id);
    let lease_secs = req.lease_secs.unwrap_or(60);
    match store
        .claim_one(
            &wid,
            &req.capabilities,
            std::time::Duration::from_secs(lease_secs),
        )
        .await
    {
        Ok(Some(claimed)) => (StatusCode::OK, Json(claimed)).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn results_handler(
    State(state): State<AppState>,
    Path((worker_id, task_id)): Path<(String, String)>,
    Json(req): Json<ResultsRequest>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    use crate::distributed::{CompletionPayload, DistributedError};
    let payload = CompletionPayload {
        task_id,
        worker_id: WorkerId::new(worker_id),
        status_str: req.status_str,
        response: req.response,
        error_msg: req.error_msg,
        usage: req.usage,
        cost_usd: req.cost_usd,
    };
    match store.complete(payload).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DistributedError::LeaseLost { .. }) => lease_lost_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn plan_results_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    match store.pull_results(&plan_id).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn extend_lease_handler(
    State(state): State<AppState>,
    Path((worker_id, task_id)): Path<(String, String)>,
    Json(req): Json<ExtendLeaseRequest>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    use crate::distributed::DistributedError;
    let wid = WorkerId::new(worker_id);
    let new_expires_at =
        chrono::Utc::now() + chrono::Duration::from_std(std::time::Duration::from_secs(req.extend_secs))
            .unwrap_or(chrono::Duration::seconds(60));
    match store.extend_lease(&wid, &task_id, new_expires_at).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DistributedError::LeaseLost { .. }) => lease_lost_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use tower::ServiceExt;

    use crate::distributed::DistributedTaskStore;
    use crate::routes::router;
    use crate::state::AppState;

    const TEST_TOKEN: &str = "test-token";

    /// Build an AppState with a fresh in-memory DistributedTaskStore.
    async fn build_test_state_with_store() -> AppState {
        let store = DistributedTaskStore::open_in_memory()
            .await
            .expect("in-memory store must initialise");
        AppState::new(Some(TEST_TOKEN.to_owned()))
            .with_distributed_store(Arc::new(store))
    }

    /// Build an AppState with NO distributed store (distributed_store == None).
    fn build_test_state_no_store() -> AppState {
        AppState::new(Some(TEST_TOKEN.to_owned()))
    }

    /// Convenience: build an authorised POST request with a JSON body.
    fn authed_post(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    /// Convenience: build an authorised GET request.
    fn authed_get(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header("Authorization", format!("Bearer {TEST_TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    fn make_subtask(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "description": format!("task {id}"),
            "agent": "test-agent",
            "context": "",
            "depends_on": [],
            "acceptance_tests": [],
            "budget_usd": null,
            "scope": [],
            "requires_capabilities": []
        })
    }

    // ── enqueue endpoint ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn enqueue_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/plan-1/enqueue",
                serde_json::json!({ "subtasks": [make_subtask("s1")] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn enqueue_with_valid_subtasks_returns_task_ids() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/plan-1/enqueue",
                serde_json::json!({ "subtasks": [make_subtask("s1")] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids = json["task_ids"].as_array().unwrap();
        assert_eq!(ids.len(), 1, "should have exactly 1 task_id");
    }

    #[tokio::test]
    async fn enqueue_with_empty_subtasks_returns_empty_task_ids() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/plan-empty/enqueue",
                serde_json::json!({ "subtasks": [] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids = json["task_ids"].as_array().unwrap();
        assert!(ids.is_empty(), "empty subtask list should produce empty task_ids");
    }

    #[tokio::test]
    async fn enqueue_without_auth_returns_401() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/distributed/plans/plan-1/enqueue")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(
                            &serde_json::json!({ "subtasks": [make_subtask("s1")] }),
                        )
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── plan status endpoint ──────────────────────────────────────────────────

    #[tokio::test]
    async fn plan_status_unknown_plan_returns_zeros() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_get("/distributed/plans/no-such-plan/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["queued"], 0);
        assert_eq!(json["leased"], 0);
        assert_eq!(json["done"], 0);
        assert_eq!(json["failed"], 0);
        assert_eq!(json["timed_out"], 0);
    }

    #[tokio::test]
    async fn plan_status_after_enqueue_reflects_counts() {
        let state = build_test_state_with_store().await;
        let app = router(state);

        // Enqueue 3 subtasks.
        app.clone()
            .oneshot(authed_post(
                "/distributed/plans/plan-count/enqueue",
                serde_json::json!({
                    "subtasks": [
                        make_subtask("s1"),
                        make_subtask("s2"),
                        make_subtask("s3"),
                    ]
                }),
            ))
            .await
            .unwrap();

        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-count/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["queued"], 3, "3 subtasks should be queued");
    }

    #[tokio::test]
    async fn plan_status_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-1/status"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── worker heartbeat ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn heartbeat_returns_204() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/heartbeat",
                serde_json::json!({ "capabilities": ["rust"], "sandbox_profile": null }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn heartbeat_creates_worker_row() {
        let state = build_test_state_with_store().await;
        // Send a heartbeat then verify the worker can claim a task (indirect
        // proof of worker row existence via the store's upsert_worker path).
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        app.oneshot(authed_post(
            "/distributed/workers/worker-hb/heartbeat",
            serde_json::json!({ "capabilities": ["rust"] }),
        ))
        .await
        .unwrap();

        // If the row was created, claim_one will succeed on a queued task.
        store
            .enqueue("plan-hb", vec![hone_core::task_plan::SubTask {
                id: "t1".to_owned(),
                description: "test".to_owned(),
                agent: "test-agent".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            }])
            .await
            .unwrap();

        let wid = WorkerId::new("worker-hb");
        let claimed = store
            .claim_one(&wid, &["rust".to_owned()], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(claimed.is_some(), "heartbeat should register worker; claim should succeed");
    }

    #[tokio::test]
    async fn heartbeat_with_sandbox_profile_persists_it() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        let profile = r#"{"profile":"strict"}"#;
        app.oneshot(authed_post(
            "/distributed/workers/worker-sb/heartbeat",
            serde_json::json!({
                "capabilities": ["python"],
                "sandbox_profile": profile,
            }),
        ))
        .await
        .unwrap();

        // Verify the sandbox_profile was stored (access the pool via the
        // internal query; DistributedTaskStore does not expose a read-worker API).
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, sandbox_profile FROM distributed_workers WHERE id = 'worker-sb'",
        )
        .fetch_optional(store.pool_for_test())
        .await
        .unwrap();

        let (_, sandbox) = row.expect("worker row must exist after heartbeat");
        assert_eq!(
            sandbox.as_deref(),
            Some(profile),
            "sandbox_profile should be persisted"
        );
    }

    #[tokio::test]
    async fn heartbeat_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/heartbeat",
                serde_json::json!({ "capabilities": [] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── worker claim ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn claim_with_no_eligible_task_returns_204() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/claim",
                serde_json::json!({ "capabilities": [] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn claim_with_eligible_task_returns_claimed_task() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-claim",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "claim me".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/claim",
                serde_json::json!({ "capabilities": [] }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["task_id"].is_string(), "claimed task must have a task_id");
        assert_eq!(json["plan_id"], "plan-claim");
    }

    #[tokio::test]
    async fn claim_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/claim",
                serde_json::json!({ "capabilities": [] }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn claim_lease_secs_override_used_when_provided() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-lease",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "lease test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let before = Utc::now();
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/claim",
                serde_json::json!({ "capabilities": [], "lease_secs": 30 }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // lease_expires_at must be ~30s from `before`, within 5s tolerance.
        let expires_str = json["lease_expires_at"].as_str().expect("lease_expires_at must be present");
        let expires: chrono::DateTime<Utc> = expires_str.parse().expect("must parse as RFC3339");
        let delta = (expires - before).num_seconds();
        assert!(
            (25..=35).contains(&delta),
            "lease_expires_at should be ~30s from now; delta={delta}s"
        );
    }

    // ── worker results ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn results_endpoint_marks_task_done() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        // Enqueue and claim a task.
        store
            .enqueue(
                "plan-done",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");
        let claimed = store
            .claim_one(&wid, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let task_id = &claimed.task_id;

        // POST results.
        let payload = serde_json::json!({
            "status_str": "completed",
            "response": "great output",
            "error_msg": null,
            "usage": { "input": 10, "output": 20, "cache_read": 0, "cache_creation": 0 },
            "cost_usd": 0.001,
        });

        let resp = app
            .oneshot(authed_post(
                &format!("/distributed/workers/worker-1/results/{task_id}"),
                payload,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let (queued, leased, done, failed, _) =
            store.plan_state_counts("plan-done").await.unwrap();
        assert_eq!(done, 1, "task should be marked done");
        assert_eq!(queued + leased + failed, 0);
    }

    #[tokio::test]
    async fn results_with_lease_lost_returns_409() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-conflict",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        // Worker A claims the task.
        let wid_a = WorkerId::new("worker-a");
        let claimed = store
            .claim_one(&wid_a, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let task_id = &claimed.task_id;

        // Worker B tries to post results for a task it does not own.
        let payload = serde_json::json!({
            "status_str": "completed",
            "response": "stolen",
            "error_msg": null,
            "usage": { "input": 1, "output": 1, "cache_read": 0, "cache_creation": 0 },
            "cost_usd": 0.0,
        });

        let resp = app
            .oneshot(authed_post(
                &format!("/distributed/workers/worker-b/results/{task_id}"),
                payload,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn results_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_post(
                "/distributed/workers/worker-1/results/task-abc",
                serde_json::json!({
                    "status_str": "completed",
                    "response": "ok",
                    "error_msg": null,
                    "usage": { "input": 1, "output": 1, "cache_read": 0, "cache_creation": 0 },
                    "cost_usd": 0.0,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── plan results endpoint ─────────────────────────────────────────────────

    #[tokio::test]
    async fn plan_results_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-1/results"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn plan_results_returns_empty_array_for_unknown_plan() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_get("/distributed/plans/no-such-plan/results"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("response must be a JSON array");
        assert!(arr.is_empty(), "unknown plan should yield empty results array");
    }

    #[tokio::test]
    async fn plan_results_returns_completed_tasks_for_known_plan() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        // Enqueue and complete one task directly via the store.
        store
            .enqueue(
                "plan-results",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "results test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");
        let claimed = store
            .claim_one(&wid, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();

        store
            .complete(crate::distributed::CompletionPayload {
                task_id: claimed.task_id.clone(),
                worker_id: wid.clone(),
                status_str: "completed".to_owned(),
                response: "done output".to_owned(),
                error_msg: None,
                usage: hone_proto::TokenUsage {
                    input: 10,
                    output: 20,
                    cache_read: 0,
                    cache_creation: 0,
                },
                cost_usd: 0.001,
            })
            .await
            .unwrap();

        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-results/results"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let rows: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = rows.as_array().expect("response must be a JSON array");
        assert_eq!(arr.len(), 1, "one completed task should appear");
        assert_eq!(arr[0]["status"], "completed");
        assert_eq!(arr[0]["response"], "done output");
        assert_eq!(arr[0]["agent"], "test-agent");
    }

    // ── extend-lease ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn extend_lease_extends_when_owner_matches() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-ext",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let wid = WorkerId::new("worker-1");
        let claimed = store
            .claim_one(&wid, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let task_id = &claimed.task_id;

        let resp = app
            .oneshot(authed_post(
                &format!("/distributed/workers/worker-1/extend-lease/{task_id}"),
                serde_json::json!({ "extend_secs": 120 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn extend_lease_with_owner_mismatch_returns_409() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-ext-conflict",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "test".to_owned(),
                    agent: "test-agent".to_owned(),
                    context: String::new(),
                    depends_on: vec![],
                    acceptance_tests: vec![],
                    budget_usd: None,
                    scope: vec![],
                    requires_capabilities: vec![],
                }],
            )
            .await
            .unwrap();

        let wid_a = WorkerId::new("worker-a");
        let claimed = store
            .claim_one(&wid_a, &[], Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        let task_id = &claimed.task_id;

        // Worker B tries to extend a lease it doesn't own.
        let resp = app
            .oneshot(authed_post(
                &format!("/distributed/workers/worker-b/extend-lease/{task_id}"),
                serde_json::json!({ "extend_secs": 120 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
