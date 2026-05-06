//! Integration tests for `POST /run/{request_id}/step`.
//!
//! These exercise the registry through the HTTP layer — they do NOT spin
//! up the full agent loop, because that requires a live model provider.
//! Instead we register a fake run directly via `state.run_registry`, then
//! drive the endpoint over the router and assert the wire response.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_proto::StepAction;
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
async fn step_returns_404_when_run_id_unknown() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/run/unknown-id/step",
            serde_json::json!("Proceed"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn step_returns_202_and_delivers_action_to_receiver() {
    let state = AppState::new(None);
    // Register a fake run BEFORE the request goes through.
    let (mut rx, _guard) = state
        .run_registry
        .register("fake-run-1".into())
        .expect("register fake run");

    let app = router(state);

    let response = app
        .oneshot(json_post("/run/fake-run-1/step", serde_json::json!("Skip")))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let action = rx.recv().await.expect("agent receives the action");
    assert_eq!(action, StepAction::Skip);
}

#[tokio::test]
async fn step_returns_410_when_receiver_dropped() {
    let state = AppState::new(None);
    let (rx, _guard) = state
        .run_registry
        .register("fake-run-2".into())
        .expect("register");
    drop(rx);

    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/run/fake-run-2/step",
            serde_json::json!("Proceed"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::GONE);
}

#[tokio::test]
async fn step_returns_400_when_body_is_not_a_step_action() {
    let state = AppState::new(None);
    let (_rx, _guard) = state
        .run_registry
        .register("fake-run-3".into())
        .expect("register");

    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/run/fake-run-3/step",
            serde_json::json!({ "not_a_step_action": true }),
        ))
        .await
        .unwrap();

    assert!(
        matches!(response.status(), StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST),
        "expected 400/422 for malformed body, got {}",
        response.status(),
    );
}

#[tokio::test]
async fn step_with_approve_always_payload_round_trips() {
    let state = AppState::new(None);
    let (mut rx, _guard) = state
        .run_registry
        .register("fake-run-4".into())
        .expect("register");

    let app = router(state);

    let response = app
        .oneshot(json_post(
            "/run/fake-run-4/step",
            serde_json::json!({ "ApproveAlways": "shell" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    match rx.recv().await.expect("got action") {
        StepAction::ApproveAlways(name) => assert_eq!(name, "shell"),
        other => panic!("expected ApproveAlways, got {other:?}"),
    }
}
