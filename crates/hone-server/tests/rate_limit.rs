use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, RateLimiter, router};
use tower::ServiceExt;

fn health_request() -> Request<Body> {
    Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap()
}

/// Requests under the limit all return 200.
#[tokio::test]
async fn test_rate_limit_middleware_allows_under_limit() {
    let limiter = RateLimiter::new(100, Duration::from_secs(60));
    let state = AppState::new(None).with_rate_limiter(limiter);

    for i in 1..=5 {
        let app = router(state.clone());
        let response = app.oneshot(health_request()).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "request {i} should be allowed"
        );
    }
}

/// When ConnectInfo is absent (no real TCP connection), all requests share
/// the "unknown" fallback bucket. The 3rd request past a limit of 2 is rejected.
#[tokio::test]
async fn test_rate_limit_middleware_blocks_over_limit() {
    let limiter = RateLimiter::new(2, Duration::from_secs(60));
    let state = AppState::new(None).with_rate_limiter(limiter);

    // First two requests succeed (same "unknown" bucket).
    for i in 1..=2 {
        let app = router(state.clone());
        let response = app.oneshot(health_request()).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "request {i} should be allowed"
        );
    }

    // Third request must be rejected.
    let app = router(state.clone());
    let response = app.oneshot(health_request()).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "3rd request should be rate-limited"
    );
}

/// Per-client isolation is tested at the RateLimiter level (not middleware)
/// because `tower::ServiceExt::oneshot` does not populate ConnectInfo.
/// The middleware correctly delegates to RateLimiter::check which is
/// thoroughly tested in the inline unit tests in rate_limit.rs.
#[tokio::test]
async fn test_rate_limiter_per_client_isolation() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60));

    // Client A exhausts its budget.
    assert!(limiter.check("192.168.1.1").await, "A first ok");
    assert!(!limiter.check("192.168.1.1").await, "A second blocked");

    // Client B still has its own budget.
    assert!(limiter.check("192.168.1.2").await, "B first ok");
    assert!(!limiter.check("192.168.1.2").await, "B second blocked");
}

/// Without ConnectInfo, the middleware falls back to "unknown" — all
/// requests share one bucket regardless of X-Forwarded-For.
#[tokio::test]
async fn test_rate_limit_middleware_ignores_forwarded_for() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60));
    let state = AppState::new(None).with_rate_limiter(limiter);

    // First request with "IP A" via X-Forwarded-For.
    let app = router(state.clone());
    let req = Request::builder()
        .uri("/health")
        .header("x-forwarded-for", "10.0.0.1")
        .body(Body::empty())
        .unwrap();
    let r1 = app.oneshot(req).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);

    // Second request with "IP B" — still hits the same "unknown" bucket.
    let app = router(state.clone());
    let req = Request::builder()
        .uri("/health")
        .header("x-forwarded-for", "10.0.0.2")
        .body(Body::empty())
        .unwrap();
    let r2 = app.oneshot(req).await.unwrap();
    assert_eq!(
        r2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "X-Forwarded-For should NOT create separate buckets — all share 'unknown'"
    );
}
