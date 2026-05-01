//! RED-PHASE regression tests for **MEDIUM-12**: `/sessions/gc?older_than_days=...`
//! computes `days * 86_400` without overflow protection.
//!
//! Audit finding (h):
//!     `crates/hone-server/src/routes.rs:183`
//!         let cutoff = now_unix().saturating_sub(days * 86_400);
//!     With `days = u64::MAX`, the unchecked multiplication panics in debug
//!     builds and silently wraps in release.  The downstream
//!     `saturating_sub` then clamps to 0, producing a "delete all sessions"
//!     GC sweep — the polar opposite of the operator's intent.
//!
//! Contract these tests pin:
//!     The handler MUST NOT panic on adversarial query input.  For an
//!     extremely large `older_than_days` it must respond with a successful
//!     status (the SQLite store handles `cutoff = 0` by deleting nothing,
//!     which is the conservative reading of "no session is older than the
//!     beginning of time").
//!
//! These tests will FAIL against current `main` because:
//!     - cargo test profile inherits debug overflow checks → the
//!       multiplication panics → axum surfaces it as `500 INTERNAL SERVER
//!       ERROR` (or a connection drop, depending on tower's panic config).
//!
//! Implementer's freedom (green phase):
//!     - Replace `days * 86_400` with `days.saturating_mul(86_400)`.
//!     - Optionally cap `days` at a sane upper bound (e.g. 365 * 100) and
//!       return 400 BAD REQUEST for absurd values.  Either is acceptable;
//!       the only forbidden behaviour is the panic.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

#[tokio::test]
async fn gc_does_not_panic_on_u64_max_days() {
    let state = AppState::new(None);
    let app = router(state);

    // u64::MAX * 86_400 overflows; a saturating_mul produces u64::MAX,
    // which the saturating_sub then turns into cutoff = 0.
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/sessions/gc?older_than_days=18446744073709551615")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    assert!(
        !status.is_server_error(),
        "AUDIT-MEDIUM-12: /sessions/gc must not panic (and therefore must \
         not return 5xx) on adversarial older_than_days values.  Use \
         saturating_mul.  Got: {status}"
    );
    // Positive contract: the request should succeed with a sensible result.
    assert_eq!(
        status,
        StatusCode::OK,
        "AUDIT-MEDIUM-12: gc with maximally-large older_than_days must \
         return 200 OK with `removed = 0` (cutoff saturates to 0, no \
         session is older than the epoch).  Got: {status}"
    );
}

#[tokio::test]
async fn gc_does_not_panic_on_near_overflow_days() {
    // The audit example: a value just below u64::MAX still overflows when
    // multiplied by 86_400.  The exact threshold is u64::MAX / 86_400 =
    // 213_503_982_334_601, so 213_503_982_334_602 is the smallest value
    // that overflows.  Pin the boundary.
    const OVERFLOW_THRESHOLD: u64 = u64::MAX / 86_400 + 1;

    let state = AppState::new(None);
    let app = router(state);

    let uri = format!("/sessions/gc?older_than_days={OVERFLOW_THRESHOLD}");
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    assert!(
        !status.is_server_error(),
        "AUDIT-MEDIUM-12: gc must not panic at the smallest overflowing \
         days value ({OVERFLOW_THRESHOLD}).  Got: {status}"
    );
}

#[tokio::test]
async fn gc_default_request_still_works_after_fix() {
    // Regression guard: the saturating-multiply fix must not break the
    // happy path.  An unparameterised request should default to 30 days
    // and return 200.
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/sessions/gc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "regression: default /sessions/gc (older_than_days=30) must remain 200"
    );
}
