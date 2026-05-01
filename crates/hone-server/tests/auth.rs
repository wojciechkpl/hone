//! Auth-middleware contract tests.
//!
//! Updated during RED-PHASE for **MAJOR-7**: the previous version of
//! `test_auth_token_required` asserted that `/health` returns 401 without a
//! token — pinning the audit bug AS the desired contract.  After the fix
//! `/health` is auth-exempt (so liveness probes and load balancers can hit
//! it without a token), so the auth-required assertion must move to a real
//! protected endpoint.  We use `POST /sessions` (creating a session is
//! clearly admin-level and remains behind auth).
//!
//! See also: `health_no_auth.rs` for the new `/health` is-public assertion.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

#[tokio::test]
async fn test_auth_token_required_on_protected_endpoint() {
    let state = AppState::new(Some("secret-token".to_owned()));
    let app = router(state);

    // POST /sessions is a real protected endpoint — creating a session
    // requires auth even after /health is moved to the public router.
    let no_auth_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        no_auth_response.status(),
        StatusCode::UNAUTHORIZED,
        "POST /sessions without a bearer token must return 401 when auth is configured"
    );

    // With the correct bearer token, POST /sessions returns 201 CREATED.
    let authed_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("Authorization", "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        authed_response.status(),
        StatusCode::CREATED,
        "POST /sessions with a valid token must succeed; got {}",
        authed_response.status()
    );
}
