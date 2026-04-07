use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{router, AppState};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState::new(None)
}

#[tokio::test]
async fn test_health_endpoint() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_post_sessions_creates_session() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["session_id"].is_string(), "expected session_id field");
}

#[tokio::test]
async fn test_get_session_returns_info() {
    let state = make_state();

    // Create a session via the store directly so we have its id.
    let info = state.sessions.create_new().await.unwrap();
    let session_id = info.id.to_string();

    let app = router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], session_id);
}

#[tokio::test]
async fn test_get_missing_session_returns_404() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/sessions/nonexistent-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_session() {
    let state = make_state();
    let info = state.sessions.create_new().await.unwrap();
    let session_id = info.id.to_string();

    // DELETE the session.
    let delete_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // GET should now return 404.
    let get_response = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);
}
