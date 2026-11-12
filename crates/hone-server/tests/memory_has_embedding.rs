//! M10 of `docs/plans/memory-recall-index-spec.md`, at the HTTP route: `GET
//! /memory` reports `has_embedding: true` for a memory stored with a vector in a
//! SQLite store (the lean BM25 corpus query must not leak into the listing).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_memory::{Memory, MemoryKind, MemoryStore, SqliteStore};
use hone_server::{AppState, router};
use tower::ServiceExt;

fn memory(id: &str, embedding: Option<Vec<f32>>) -> Memory {
    Memory {
        id: id.to_string(),
        content: format!("content of {id}"),
        tags: vec![],
        weight: 1.0,
        created_at: 1,
        decay_at: None,
        kind: MemoryKind::Feedback,
        description: None,
        embedding,
    }
}

#[tokio::test]
async fn test_m10_memory_list_should_report_has_embedding_for_a_stored_vector() {
    let store = Arc::new(SqliteStore::in_memory().await.unwrap());
    store.record(memory("with", Some(vec![0.5, 0.5]))).await.unwrap();
    store.record(memory("without", None)).await.unwrap();
    let app = router(AppState::new(None).with_memory_store(store));

    let response = app
        .oneshot(Request::builder().uri("/memory").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024).await.unwrap();
    let list: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    let flag = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from {list}"))["has_embedding"]
            .as_bool()
            .unwrap()
    };
    assert!(flag("with"), "a stored vector must be reported");
    assert!(!flag("without"));
}
