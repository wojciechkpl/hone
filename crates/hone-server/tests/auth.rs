use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{router, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn test_auth_token_required() {
    let state = AppState::new(Some("secret-token".to_owned()));
    let app = router(state);

    // Request without a token should return 401.
    let no_auth_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(no_auth_response.status(), StatusCode::UNAUTHORIZED);

    // Request with the correct bearer token should return 200.
    let authed_response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("Authorization", "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(authed_response.status(), StatusCode::OK);
}
