//! Integration tests for `GET /agents` and `GET /recipes`.

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
async fn agents_endpoint_returns_array_with_at_least_builtins() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/agents").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .starts_with("application/json"),
        "expected JSON content-type",
    );

    let arr = body_json(response).await;
    assert!(arr.is_array(), "agents endpoint must return JSON array");
    let arr = arr.as_array().unwrap();
    assert!(!arr.is_empty(), "must include built-in agents");
    let first = &arr[0];
    assert!(first.get("name").is_some());
    assert!(first.get("description").is_some());
    assert!(first.get("model").is_some());
    assert!(first.get("source").is_some());
}

#[tokio::test]
async fn recipes_endpoint_returns_array() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/recipes").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let arr = body_json(response).await;
    assert!(arr.is_array(), "recipes endpoint must return JSON array");
    // We don't assert non-empty — the test runs with a temp workspace_root
    // by default, so there may be no recipes. We just check shape.
}

#[tokio::test]
async fn agents_endpoint_each_entry_has_source_tag() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/agents").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let arr = body_json(response).await;
    for entry in arr.as_array().unwrap() {
        let source = entry
            .get("source")
            .and_then(|s| s.as_str())
            .expect("source field must be a string");
        assert!(
            matches!(source, "builtin" | "user" | "project"),
            "unexpected source tag: {source}",
        );
    }
}

#[tokio::test]
async fn agent_detail_endpoint_returns_full_prompt_for_builtin() {
    let state = AppState::new(None);
    let app = router(state.clone());

    // Discover the name of any built-in via the list endpoint, then ask
    // for its detail.
    let list_resp = app
        .oneshot(Request::builder().uri("/agents").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let arr = body_json(list_resp).await;
    let first = arr
        .as_array()
        .and_then(|a| a.first())
        .expect("at least one builtin");
    let name = first.get("name").and_then(|v| v.as_str()).unwrap();

    let app2 = router(state);
    let detail_resp = app2
        .oneshot(
            Request::builder()
                .uri(format!("/agents/{name}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), StatusCode::OK);
    let detail = body_json(detail_resp).await;
    assert_eq!(detail.get("name").and_then(|v| v.as_str()), Some(name));
    let prompt = detail
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .expect("system_prompt is a string");
    assert!(!prompt.is_empty(), "builtin must have non-empty body");
}

#[tokio::test]
async fn agent_detail_endpoint_returns_404_for_unknown_name() {
    let state = AppState::new(None);
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/agents/no-such-agent-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn recipe_detail_endpoint_returns_404_for_unknown_name() {
    let state = AppState::new(None);
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/recipes/no-such-recipe-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
