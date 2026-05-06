//! Integration tests for `GET /mcp/marketplace` and `GET /mcp/installed`.

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
async fn marketplace_endpoint_returns_curated_array() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/mcp/marketplace")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let arr = body.as_array().expect("array");
    assert!(arr.len() >= 5, "expected >= 5 curated entries, got {}", arr.len());

    // Spot-check the filesystem entry shape.
    let fs = arr
        .iter()
        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some("filesystem"))
        .expect("filesystem entry");
    assert_eq!(fs.get("command").and_then(|v| v.as_str()), Some("npx"));
    assert!(fs.get("args").and_then(|v| v.as_array()).is_some());
    assert!(fs.get("source_url").and_then(|v| v.as_str()).is_some());
    assert!(fs.get("install_hint").and_then(|v| v.as_str()).is_some());
}

#[tokio::test]
async fn marketplace_each_entry_has_required_fields() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/mcp/marketplace")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(response).await;
    for entry in body.as_array().unwrap() {
        for required in ["id", "name", "description", "command", "args", "source_url", "install_hint"] {
            assert!(
                entry.get(required).is_some(),
                "missing required field {required} in entry: {entry}",
            );
        }
        // official is optional in the wire format but always present in the
        // builtin (defaults to false on absence).
        assert!(entry.get("official").and_then(|v| v.as_bool()).is_some());
    }
}

#[tokio::test]
async fn installed_endpoint_returns_array_of_servers() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/mcp/installed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body.is_array(), "must return JSON array");
    // Body may be empty or non-empty depending on host config; only check
    // the shape of the source tag for any present entries.
    for entry in body.as_array().unwrap() {
        let source = entry
            .get("source")
            .and_then(|s| s.as_str())
            .expect("source must be a string");
        assert!(
            matches!(source, "global" | "project"),
            "unexpected source tag: {source}",
        );
    }
}
