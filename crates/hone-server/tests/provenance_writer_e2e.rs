//! End-to-end integration test for the provenance writer wired into
//! `POST /run`. Drives a turn through a `MockProvider` that emits a few
//! tokens then `Done`, and asserts the chain on disk verifies + has
//! every expected entry.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_proto::TokenUsage;
use hone_provenance::{ProvenanceEvent, ProvenanceLog};
use hone_providers::{MockProvider, ModelProvider};
use hone_server::{AppState, router};
use tempfile::TempDir;
use tower::ServiceExt;

fn mock_provider() -> Arc<dyn ModelProvider> {
    let usage = TokenUsage { input: 10, output: 20, cache_read: 0, cache_creation: 0 };
    Arc::new(
        MockProvider::new(vec!["hello".into(), " from".into(), " mock".into()])
            .with_usage(usage),
    )
}

fn run_request(prompt: &str, request_id: Option<&str>) -> Body {
    let mut body = serde_json::json!({ "prompt": prompt });
    if let Some(id) = request_id {
        body["request_id"] = serde_json::Value::String(id.into());
    }
    Body::from(body.to_string())
}

#[tokio::test]
async fn run_with_provenance_dir_writes_a_verifiable_chain() {
    let tmp = TempDir::new().expect("tmp");
    let state = AppState::new(None)
        .with_mock_provider(mock_provider())
        .with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/run")
                .method("POST")
                .header("content-type", "application/json")
                .body(run_request("do a thing", Some("test-session-1")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Drain the SSE stream so the unfold runs the tee on every event.
    let _ = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();

    // Re-open the chain file and verify it.
    let path = tmp.path().join("test-session-1.jsonl");
    assert!(path.is_file(), "provenance log was not created at {}", path.display());
    let log = ProvenanceLog::open(&path).expect("chain reopens + verifies on load");
    assert!(log.entries().len() >= 3, "expected at least prompt + completion + session_end");

    // First entry must be the user prompt.
    let first = &log.entries()[0];
    match &first.event {
        ProvenanceEvent::UserPrompt { text } => assert_eq!(text, "do a thing"),
        other => panic!("expected UserPrompt at index 0, got {other:?}"),
    }
    // Last entry must be a SessionEnd (the mapper closes every chain that way).
    match &log.entries().last().unwrap().event {
        ProvenanceEvent::SessionEnd { .. } => {}
        other => panic!("expected SessionEnd at last index, got {other:?}"),
    }
    // The aggregated tokens must produce a single ModelCompletion in between.
    let has_completion = log.entries().iter().any(|e| {
        matches!(&e.event, ProvenanceEvent::ModelCompletion { output, .. } if output == "hello from mock")
    });
    assert!(has_completion, "expected a ModelCompletion entry with the joined token stream");
}

#[tokio::test]
async fn run_without_provenance_dir_writes_nothing() {
    let tmp = TempDir::new().expect("tmp");
    // Note: NO with_provenance_dir on this state.
    let state = AppState::new(None).with_mock_provider(mock_provider());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/run")
                .method("POST")
                .header("content-type", "application/json")
                .body(run_request("does not matter", None))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();

    // tmp is a clean directory — nothing should land there.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .expect("read tmp")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "no provenance dir was configured but {} entries were written: {entries:?}",
        entries.len(),
    );
}

#[tokio::test]
async fn provenance_dir_session_id_reuses_request_id() {
    let tmp = TempDir::new().expect("tmp");
    let state = AppState::new(None)
        .with_mock_provider(mock_provider())
        .with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/run")
                .method("POST")
                .header("content-type", "application/json")
                .body(run_request("hi", Some("custom-id")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();

    // The chain must land at <session_id>.jsonl using the request_id.
    let path = tmp.path().join("custom-id.jsonl");
    assert!(
        path.is_file(),
        "expected log at custom-id.jsonl; tmp contains: {:?}",
        std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .collect::<Vec<_>>(),
    );
}
