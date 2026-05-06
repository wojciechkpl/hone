//! Integration tests for `GET /memory` and `GET /memory/stats`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_server::{AppState, router};
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

fn fake_memory(id: &str, kind: MemoryKind, tags: &[&str], content: &str) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        weight: 1.0,
        created_at: 0,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

#[tokio::test]
async fn memory_list_returns_503_when_no_store() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn memory_stats_returns_503_when_no_store() {
    let state = AppState::new(None);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/memory/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn memory_list_returns_recorded_entries_when_store_attached() {
    let store = Arc::new(InMemoryStore::new());
    store
        .record(fake_memory("m1", MemoryKind::User, &["rust"], "x"))
        .await
        .expect("record");
    store
        .record(fake_memory("m2", MemoryKind::Project, &["rust", "perf"], "y"))
        .await
        .expect("record");

    let state = AppState::new(None).with_memory_store(store);
    let app = router(state);

    let response = app
        .oneshot(Request::builder().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let arr = body_json(response).await;
    let list = arr.as_array().expect("array");
    assert_eq!(list.len(), 2);
    for entry in list {
        assert!(entry.get("id").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("content").and_then(|v| v.as_str()).is_some());
        assert!(entry.get("kind").is_some());
        assert!(entry.get("has_embedding").and_then(|v| v.as_bool()).is_some());
    }
}

#[tokio::test]
async fn memory_stats_returns_total_and_by_kind() {
    let store = Arc::new(InMemoryStore::new());
    store
        .record(fake_memory("a", MemoryKind::User, &["rust"], "x"))
        .await
        .expect("record");
    store
        .record(fake_memory("b", MemoryKind::User, &["rust", "perf"], "y"))
        .await
        .expect("record");
    store
        .record(fake_memory("c", MemoryKind::Project, &["style"], "z"))
        .await
        .expect("record");

    let state = AppState::new(None).with_memory_store(store);
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/memory/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert_eq!(body.get("total").and_then(|v| v.as_u64()), Some(3));
    let by_kind = body.get("by_kind").expect("by_kind");
    assert_eq!(by_kind.get("user").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(by_kind.get("project").and_then(|v| v.as_u64()), Some(1));
    let top_tags = body.get("top_tags").and_then(|v| v.as_array()).expect("top_tags array");
    assert!(!top_tags.is_empty(), "tags must include 'rust'");
}
