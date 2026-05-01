//! RED-PHASE regression tests for **MAJOR-7**: `/health` is currently behind
//! `auth_middleware`, so liveness probes and load balancers receive 401
//! when an auth token is configured.
//!
//! Audit finding (f):
//!     `crates/hone-server/src/routes.rs:22-44`
//!     `/health` is registered as a regular route in `router()` and then
//!     `auth_middleware` is applied via `.layer()` — Axum processes layers
//!     outside-in, so auth fires before the handler regardless of route
//!     registration order.
//!
//! Contract these tests pin:
//!     `/health` (and `/openapi.json`, which is similarly diagnostic) MUST
//!     return `200 OK` for unauthenticated callers EVEN WHEN an auth token
//!     is configured on the daemon.  This is the standard pattern for
//!     liveness/readiness probes in Kubernetes, ECS, and any LB-fronted
//!     deployment.
//!
//! These tests will FAIL against current `main` because:
//!     - The pre-fix router applies `auth_middleware` as a top-level
//!       `.layer()` covering every route including `/health`.
//!     - With auth configured, an Authorization-less request to `/health`
//!       returns 401.
//!
//! Implementer's freedom (green phase):
//!     - Split the router into a public sub-router (`/health`,
//!       `/openapi.json`) and a protected sub-router (everything else),
//!       then `.merge()` the public one onto the auth-layered one.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

#[tokio::test]
async fn health_returns_200_without_auth_when_token_is_configured() {
    let state = AppState::new(Some("secret-token".to_owned()));
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "AUDIT-MAJOR-7: /health must be reachable without an Authorization \
         header, even when auth_token is configured.  Otherwise liveness \
         probes from load balancers / Kubernetes lock the operator out. \
         Got: {}",
        response.status()
    );
}

#[tokio::test]
async fn health_still_returns_200_with_auth_when_token_is_configured() {
    // Defensive: a well-formed request with the correct token must STILL
    // succeed.  Some implementations of the fix accidentally reject
    // authenticated requests to public endpoints; pin the negative case.
    let state = AppState::new(Some("secret-token".to_owned()));
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("Authorization", "Bearer secret-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "regression: /health with valid auth must still return 200"
    );
}

#[tokio::test]
async fn health_returns_200_without_auth_when_no_token_configured() {
    // Pre-existing behaviour: without auth configured, /health is open.
    // Pinning this prevents the fix from accidentally tightening the
    // unauthenticated-deployment path.
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "regression: /health on a no-auth deployment must remain 200"
    );
}

#[tokio::test]
async fn openapi_json_is_also_auth_exempt() {
    // The audit explicitly mentions `/openapi.json` as the second public
    // diagnostic endpoint.  Pin the same contract so a future refactor that
    // makes the public router doesn't accidentally re-protect this one.
    let state = AppState::new(Some("secret-token".to_owned()));
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "AUDIT-MAJOR-7: /openapi.json is a public-by-convention diagnostic \
         endpoint and must not require auth.  Got: {}",
        response.status()
    );
}
