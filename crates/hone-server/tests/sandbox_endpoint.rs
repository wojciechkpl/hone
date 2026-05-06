//! Integration tests for `GET /sandbox/policies`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

#[tokio::test]
async fn sandbox_endpoint_returns_active_policy_and_overrides() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sandbox/policies")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert!(body.get("active").is_some(), "must include 'active'");
    let active = body.get("active").unwrap();
    assert!(active.get("mode").is_some());
    assert!(active.get("allow_network").is_some());
    assert!(active.get("description").is_some(), "active must have description");

    let overrides = body
        .get("agent_overrides")
        .and_then(|v| v.as_array())
        .expect("agent_overrides is an array");
    for entry in overrides {
        assert!(entry.get("agent").and_then(|v| v.as_str()).is_some());
        let policy = entry.get("policy").expect("policy field");
        assert!(policy.get("mode").is_some());
        assert!(policy.get("allow_network").is_some());
    }
}

#[tokio::test]
async fn sandbox_endpoint_active_policy_is_permissive_by_default() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/sandbox/policies")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(response).await;
    let active = body.get("active").unwrap();
    assert_eq!(active.get("mode").and_then(|v| v.as_str()), Some("manual"));
    assert_eq!(active.get("allow_network").and_then(|v| v.as_bool()), Some(true));
    assert!(active.get("allow_read").unwrap().as_array().unwrap().is_empty());
    assert!(active.get("allow_write").unwrap().as_array().unwrap().is_empty());
}
