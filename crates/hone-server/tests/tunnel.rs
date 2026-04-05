use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{router, AppState, MockTunnelManager};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState::with_tunnel_manager(None, Arc::new(MockTunnelManager::new()))
}

fn make_state_with_auth(token: &str) -> AppState {
    AppState::with_tunnel_manager(
        Some(token.to_owned()),
        Arc::new(MockTunnelManager::new()),
    )
}

fn start_tunnel_request(local_port: u16) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/tunnel")
        .header("Content-Type", "application/json")
        .body(Body::from(
            serde_json::json!({ "local_port": local_port }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn test_start_tunnel() {
    let app = router(make_state());
    let response = app
        .oneshot(start_tunnel_request(8080))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["is_active"], true);
    assert!(
        json["url"].as_str().unwrap().contains("8080"),
        "url should contain the port"
    );
}

#[tokio::test]
async fn test_tunnel_status_when_active() {
    let state = make_state();

    // Start the tunnel directly via the manager so we share state across requests.
    state
        .tunnel_manager
        .start(&hone_server::tunnel::TunnelConfig {
            local_port: 9000,
            auth_token: String::new(),
        })
        .await
        .unwrap();

    let app = router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/tunnel")
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

    assert_eq!(json["is_active"], true);
}

#[tokio::test]
async fn test_tunnel_status_when_inactive() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/tunnel")
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

    assert_eq!(json["is_active"], false);
}

#[tokio::test]
async fn test_stop_tunnel() {
    let state = make_state();
    let app = router(state.clone());

    // Start the tunnel via POST.
    app.oneshot(start_tunnel_request(7070)).await.unwrap();

    // DELETE should return 204.
    let delete_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/tunnel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // GET should now report inactive.
    let status_response = router(state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/tunnel")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(status_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(status_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["is_active"], false);
}

#[tokio::test]
async fn test_tunnel_requires_auth() {
    let app = router(make_state_with_auth("secret"));

    // POST /tunnel without Authorization header should return 401.
    let response = app
        .oneshot(start_tunnel_request(8080))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
