use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{router, ApiDoc, AppState};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState::new(None)
}

// ── Session GC endpoint ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_gc_sessions_endpoint() {
    let state = make_state();

    // Create a couple of sessions so GC has something to operate on.
    state.sessions.create_new().await.unwrap();
    state.sessions.create_new().await.unwrap();

    // DELETE /sessions/gc?older_than_days=0 uses cutoff = now - 0 days = now.
    // The GC predicate is `created_at < cutoff`, so sessions created within
    // the current second are retained (created_at == cutoff). The important
    // thing to test is that the endpoint responds 200 with a numeric `removed`
    // field — the exact count depends on sub-second timing.
    let app = router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/sessions/gc?older_than_days=0")
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
    assert!(
        json["removed"].is_number(),
        "response must have a numeric `removed` field"
    );
}

#[tokio::test]
async fn test_gc_sessions_endpoint_default_days() {
    // Without a query param, the default of 30 days is used — sessions created
    // "now" are newer than 30 days, so nothing is removed.
    let state = make_state();
    state.sessions.create_new().await.unwrap();

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

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["removed"].as_u64().unwrap(), 0);
}

// ── OpenAPI endpoint ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_openapi_json_endpoint() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
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
    assert!(
        json.get("info").is_some(),
        "OpenAPI spec must contain an `info` field"
    );
}

#[tokio::test]
async fn test_openapi_generation_does_not_panic() {
    // Ensure the macro-generated spec can be serialized without panicking.
    let json = ApiDoc::json();
    assert!(!json.is_empty());
    // Must be valid JSON.
    let _: serde_json::Value = serde_json::from_str(&json).expect("ApiDoc::json must be valid JSON");
}
