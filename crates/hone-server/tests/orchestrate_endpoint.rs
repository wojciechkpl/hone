//! Smoke tests for `POST /orchestrate`.
//!
//! Real end-to-end orchestration requires a mock provider that emits a
//! valid plan-decomposition tool call, which is brittle to the lead
//! agent's prompt template. These tests instead verify the route is
//! mounted, accepts the documented request body, and produces a
//! `text/event-stream` response. The richer end-to-end coverage lives in
//! `hone-core::orchestrator` integration tests.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

fn json_post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn orchestrate_route_is_mounted_and_returns_sse() {
    // No provider configured → handler emits a single Error event then
    // closes the stream. Status is still 200 because the SSE handshake
    // succeeded; the failure surfaces in the stream itself.
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/orchestrate",
            serde_json::json!({
                "prompt": "do a thing",
                "max_workers": 2
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        ct.starts_with("text/event-stream"),
        "expected SSE content-type, got: {ct}"
    );
}

#[tokio::test]
async fn orchestrate_rejects_malformed_body() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/orchestrate",
            serde_json::json!({ "wrong_field": 42 }),
        ))
        .await
        .unwrap();

    // axum's Json extractor returns 422 for missing required fields.
    assert!(
        matches!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST
        ),
        "expected 400/422, got {}",
        response.status(),
    );
}

#[tokio::test]
async fn orchestrate_accepts_optional_history_and_budget() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/orchestrate",
            serde_json::json!({
                "prompt": "p",
                "history": [{"role": "user", "content": "earlier"}],
                "max_workers": 3,
                "total_budget_usd": 0.5,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
