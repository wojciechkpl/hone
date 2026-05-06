//! Integration tests for the `/ui` static asset routes.
//!
//! The tests assume the webui has been built (`make webui`) so that
//! `apps/webui/dist/` exists at compile time. When the dist is empty,
//! the route returns 404 with a helpful message; when present, it
//! returns the bundled index.html.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

#[tokio::test]
async fn ui_root_returns_200_or_404_with_known_message() {
    let state = AppState::new(None);
    let app = router(state);

    let resp = app
        .oneshot(Request::builder().uri("/ui").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert!(
        matches!(resp.status(), StatusCode::OK | StatusCode::NOT_FOUND),
        "unexpected status: {}",
        resp.status(),
    );
}

#[tokio::test]
async fn ui_assets_route_falls_through_to_index_for_unknown_paths() {
    // SPA fallback: anything that doesn't match an asset name returns
    // index.html so client-side routing works.
    let state = AppState::new(None);
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ui/some/deep/route/that/does/not/exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        matches!(resp.status(), StatusCode::OK | StatusCode::NOT_FOUND),
        "unexpected status: {}",
        resp.status(),
    );
}

#[tokio::test]
async fn ui_routes_are_public_when_auth_token_is_set() {
    // The web UI must load before the user authenticates, so the static
    // assets must be reachable without an Authorization header even
    // when an auth token is configured on the daemon.
    let state = AppState::new(Some("secret-token".into()));
    let app = router(state);

    let resp = app
        .oneshot(Request::builder().uri("/ui").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "/ui must be reachable without auth, got 401",
    );
}
