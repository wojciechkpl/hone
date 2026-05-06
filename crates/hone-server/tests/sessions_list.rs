//! Integration tests for `GET /sessions` (list).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

async fn create_session(app: axum::Router) -> (axum::Router, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/sessions")
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    let id = body
        .get("session_id")
        .and_then(|v| v.as_str())
        .expect("session_id")
        .to_string();
    (app, id)
}

#[tokio::test]
async fn list_sessions_returns_empty_array_when_none_exist() {
    let state = AppState::new(None);
    let app = router(state);
    let resp = app
        .oneshot(Request::builder().uri("/sessions").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    assert!(arr.is_array());
    assert_eq!(arr.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_sessions_returns_created_sessions() {
    let state = AppState::new(None);
    let app = router(state);

    let (app, id1) = create_session(app).await;
    let (app, id2) = create_session(app).await;

    let resp = app
        .oneshot(Request::builder().uri("/sessions").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let list = arr.as_array().expect("array");
    assert_eq!(list.len(), 2);
    let ids: Vec<&str> = list.iter().filter_map(|e| e.get("id").and_then(|v| v.as_str())).collect();
    assert!(ids.contains(&id1.as_str()));
    assert!(ids.contains(&id2.as_str()));
}
