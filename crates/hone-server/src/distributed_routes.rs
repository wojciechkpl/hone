use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
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
pub struct CancelResponse {
    pub cancelled: usize,
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
    #[serde(default)]
    pub provenance_head: Option<String>,
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
        .route(
            "/distributed/plans/{plan_id}/events",
            get(plan_events_handler),
        )
        .route(
            "/distributed/plans/{plan_id}/cancel",
            post(plan_cancel_handler),
        )
        .route(
            "/distributed/plans/{plan_id}/provenance",
            get(plan_provenance_handler),
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
        provenance_head: req.provenance_head,
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

/// GET /distributed/plans/{plan_id}/events
/// SSE stream that emits task_completion events as tasks finish.
/// Closes automatically when the plan becomes fully terminal.
async fn plan_events_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };

    let store = Arc::clone(&store);
    let plan_id_owned = plan_id.clone();

    let stream = async_stream::stream! {
        use std::collections::HashSet;
        use std::time::Duration;

        let mut emitted: HashSet<String> = HashSet::new();
        let poll_interval = Duration::from_millis(200);

        loop {
            // Pull current results and emit any new ones.
            if let Ok(rows) = store.pull_results(&plan_id_owned).await {
                for row in rows {
                    if emitted.insert(row.task_id.clone()) {
                        let payload = serde_json::json!({
                            "task_id": row.task_id,
                            "status": row.status,
                        });
                        let evt = Event::default()
                            .event("task_completion")
                            .data(payload.to_string());
                        yield Ok::<_, axum::Error>(evt);
                    }
                }
            }

            // Check if plan is fully terminal (no queued or leased tasks remain).
            match store.plan_state_counts(&plan_id_owned).await {
                Ok((queued, leased, _, _, _)) if queued + leased == 0 => break,
                _ => {}
            }

            tokio::time::sleep(poll_interval).await;
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// POST /distributed/plans/{plan_id}/cancel
/// Flips all queued+leased tasks to Failed. Returns {"cancelled": N}.
async fn plan_cancel_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    match store.cancel_plan(&plan_id, "cancelled by user").await {
        Ok(count) => (
            StatusCode::OK,
            Json(serde_json::json!({ "cancelled": count })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /distributed/plans/{plan_id}/provenance
/// Returns the Merkle root and per-task provenance heads for a plan.
async fn plan_provenance_handler(
    State(state): State<AppState>,
    Path(plan_id): Path<String>,
) -> axum::response::Response {
    let Some(store) = require_store(&state) else {
        return store_unavailable();
    };
    match store.compute_plan_provenance(&plan_id).await {
        Ok(prov) => (StatusCode::OK, Json(prov)).into_response(),
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
                provenance_head: None,
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

    // ── cancel endpoint ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/plan-1/cancel",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn cancel_returns_count_in_body() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-cancel-route",
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

        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/plan-cancel-route/cancel",
                serde_json::json!({}),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["cancelled"], 1,
            "cancel should report 1 task transitioned"
        );
    }

    #[tokio::test]
    async fn cancel_with_no_queued_tasks_returns_zero() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_post(
                "/distributed/plans/no-tasks-plan/cancel",
                serde_json::json!({}),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["cancelled"], 0, "plan with no tasks should return 0");
    }

    // ── events (SSE) endpoint ─────────────────────────────────────────────────

    /// Collect all bytes from the SSE response body within a timeout.
    /// Returns the raw bytes accumulated before the stream closes.
    async fn collect_sse_bytes(body: axum::body::Body, timeout_ms: u64) -> Vec<u8> {
        use tokio::time::{Duration, timeout};
        // Collect with a generous upper bound; SSE streams are short in tests.
        let result = timeout(
            Duration::from_millis(timeout_ms),
            axum::body::to_bytes(body, usize::MAX),
        )
        .await;
        match result {
            Ok(Ok(b)) => b.to_vec(),
            _ => vec![],
        }
    }

    /// Parse SSE frames from raw bytes. Returns a vec of (event_name, data) pairs.
    fn parse_sse_frames(raw: &[u8]) -> Vec<(String, String)> { // raw is &[u8] — sized OK
        let text = std::str::from_utf8(raw).unwrap_or("");
        let mut frames = vec![];
        let mut current_event = String::new();
        let mut current_data = String::new();
        let mut in_frame = false;

        for line in text.lines() {
            if line.starts_with("event:") {
                current_event = line.trim_start_matches("event:").trim().to_owned();
                in_frame = true;
            } else if line.starts_with("data:") {
                current_data = line.trim_start_matches("data:").trim().to_owned();
                in_frame = true;
            } else if line.is_empty() && in_frame {
                if !current_data.is_empty() {
                    frames.push((current_event.clone(), current_data.clone()));
                }
                current_event.clear();
                current_data.clear();
                in_frame = false;
            }
        }

        frames
    }

    #[tokio::test]
    async fn events_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-1/events"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn events_emits_completion_for_already_done_task() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        // Enqueue and complete one task before the subscriber connects.
        let ids = store
            .enqueue(
                "plan-sse-done",
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
        let task_id = ids[0].clone();

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
                response: "done".to_owned(),
                error_msg: None,
                usage: hone_proto::TokenUsage {
                    input: 1,
                    output: 1,
                    cache_read: 0,
                    cache_creation: 0,
                },
                cost_usd: 0.0,
                provenance_head: None,
            })
            .await
            .unwrap();

        // Now subscribe — the plan is already terminal.
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-sse-done/events"))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(ct.contains("text/event-stream"), "content-type must be text/event-stream");

        let raw = collect_sse_bytes(resp.into_body(), 3000).await;
        let frames = parse_sse_frames(&raw);

        assert!(
            !frames.is_empty(),
            "should have received at least one SSE frame for the completed task"
        );

        // Find a frame whose data mentions our task_id with status "completed".
        let matching = frames.iter().any(|(_ev, data)| {
            let v: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            v["task_id"].as_str() == Some(&task_id) && v["status"].as_str() == Some("completed")
        });
        assert!(
            matching,
            "expected an SSE event with task_id={task_id} status=completed; frames={frames:?}"
        );
    }

    #[tokio::test]
    async fn events_keeps_connection_open_until_terminal_then_closes() {
        use std::sync::Arc;
        use tokio::time::{Duration, timeout};

        let state = build_test_state_with_store().await;
        let store = Arc::new(state.distributed_store.clone().unwrap());
        let app = router(state);

        // Enqueue 2 tasks.
        let ids = store
            .enqueue(
                "plan-sse-stream",
                vec![
                    hone_core::task_plan::SubTask {
                        id: "s1".to_owned(),
                        description: "first".to_owned(),
                        agent: "test-agent".to_owned(),
                        context: String::new(),
                        depends_on: vec![],
                        acceptance_tests: vec![],
                        budget_usd: None,
                        scope: vec![],
                        requires_capabilities: vec![],
                    },
                    hone_core::task_plan::SubTask {
                        id: "s2".to_owned(),
                        description: "second".to_owned(),
                        agent: "test-agent".to_owned(),
                        context: String::new(),
                        depends_on: vec![],
                        acceptance_tests: vec![],
                        budget_usd: None,
                        scope: vec![],
                        requires_capabilities: vec![],
                    },
                ],
            )
            .await
            .unwrap();

        // Complete both tasks before connecting so the stream must emit 2 events
        // and then close (plan is terminal from the subscriber's perspective).
        let wid = WorkerId::new("worker-sse");
        for _ in 0..2 {
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
                    response: "ok".to_owned(),
                    error_msg: None,
                    usage: hone_proto::TokenUsage {
                        input: 1,
                        output: 1,
                        cache_read: 0,
                        cache_creation: 0,
                    },
                    cost_usd: 0.0,
                    provenance_head: None,
                })
                .await
                .unwrap();
        }

        // Subscribe to the event stream — it should close once it detects the plan is terminal.
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-sse-stream/events"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Stream must terminate (close) within 5 seconds.
        let raw = timeout(Duration::from_secs(5), collect_sse_bytes(resp.into_body(), 4500))
            .await
            .expect("SSE stream did not close within 5s — handler may not be sending EOS");

        let frames = parse_sse_frames(&raw);
        // Must have exactly 2 task_completion events (one per task).
        let completion_events: Vec<_> = frames
            .iter()
            .filter(|(ev, _)| ev == "task_completion" || ev.is_empty())
            .filter(|(_, data)| {
                let v: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
                v["status"].as_str() == Some("completed")
            })
            .collect();

        assert_eq!(
            completion_events.len(),
            2,
            "expected 2 completion events for 2 tasks; got {}: frames={frames:?}",
            completion_events.len()
        );

        // Both our task_ids must appear.
        let seen_ids: Vec<String> = completion_events
            .iter()
            .filter_map(|(_, data)| {
                let v: serde_json::Value = serde_json::from_str(data).ok()?;
                v["task_id"].as_str().map(String::from)
            })
            .collect();
        for id in &ids {
            assert!(
                seen_ids.contains(id),
                "task_id {id} missing from SSE events; seen={seen_ids:?}"
            );
        }
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

    // ── provenance endpoint ───────────────────────────────────────────────────

    #[tokio::test]
    async fn provenance_with_no_store_returns_503() {
        let app = router(build_test_state_no_store());
        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-1/provenance"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn provenance_returns_genesis_for_unknown_plan() {
        let app = router(build_test_state_with_store().await);
        let resp = app
            .oneshot(authed_get("/distributed/plans/no-such-plan/provenance"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["merkle_root"], "0".repeat(64));
        let heads = json["per_task_heads"].as_object().expect("per_task_heads must be object");
        assert!(heads.is_empty(), "unknown plan must have empty per_task_heads");
    }

    #[tokio::test]
    async fn provenance_returns_root_and_heads_for_known_plan() {
        let state = build_test_state_with_store().await;
        let store = state.distributed_store.clone().unwrap();
        let app = router(state);

        store
            .enqueue(
                "plan-prov-route",
                vec![hone_core::task_plan::SubTask {
                    id: "s1".to_owned(),
                    description: "prov test".to_owned(),
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
        let claimed = store.claim_one(&wid, &[], std::time::Duration::from_secs(60)).await.unwrap().unwrap();
        let head = "a".repeat(64);
        store.complete(crate::distributed::CompletionPayload {
            task_id: claimed.task_id.clone(),
            worker_id: wid.clone(),
            status_str: "completed".to_owned(),
            response: "done".to_owned(),
            error_msg: None,
            usage: hone_proto::TokenUsage { input: 1, output: 1, cache_read: 0, cache_creation: 0 },
            cost_usd: 0.0,
            provenance_head: Some(head.clone()),
        }).await.unwrap();

        let resp = app
            .oneshot(authed_get("/distributed/plans/plan-prov-route/provenance"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Single leaf -> root == leaf
        assert_eq!(json["merkle_root"], head.as_str());
        let heads = json["per_task_heads"].as_object().expect("must be object");
        assert_eq!(heads.len(), 1);
        let stored_head = heads.values().next().unwrap().as_str().unwrap();
        assert_eq!(stored_head, head);
    }
}
