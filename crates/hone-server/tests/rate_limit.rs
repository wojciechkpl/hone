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

fn health_request_with_ip(ip: &str) -> Request<Body> {
    Request::builder()
        .uri("/health")
        .header("x-forwarded-for", ip)
        .body(Body::empty())
        .unwrap()
}

/// 5 requests under a 100/min limit all return 200.
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

/// The 3rd request with the same IP is rejected when limit is 2/min.
#[tokio::test]
async fn test_rate_limit_middleware_blocks_over_limit() {
    let limiter = RateLimiter::new(2, Duration::from_secs(60));
    let state = AppState::new(None).with_rate_limiter(limiter);
    let ip = "10.0.0.1";

    // First two requests must succeed.
    for i in 1..=2 {
        let app = router(state.clone());
        let response = app
            .oneshot(health_request_with_ip(ip))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "request {i} should be allowed"
        );
    }

    // Third request must be rejected.
    let app = router(state.clone());
    let response = app
        .oneshot(health_request_with_ip(ip))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "3rd request should be rate-limited"
    );
}

/// Different client IPs have independent budgets.
#[tokio::test]
async fn test_rate_limit_middleware_different_clients_independent() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60));
    let state = AppState::new(None).with_rate_limiter(limiter);

    // client A exhausts its budget.
    let app = router(state.clone());
    let r1 = app
        .oneshot(health_request_with_ip("192.168.1.1"))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::OK, "client A first request ok");

    let app = router(state.clone());
    let r2 = app
        .oneshot(health_request_with_ip("192.168.1.1"))
        .await
        .unwrap();
    assert_eq!(
        r2.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "client A second request blocked"
    );

    // client B still has its own fresh budget.
    let app = router(state.clone());
    let r3 = app
        .oneshot(health_request_with_ip("192.168.1.2"))
        .await
        .unwrap();
    assert_eq!(r3.status(), StatusCode::OK, "client B first request ok");
}
