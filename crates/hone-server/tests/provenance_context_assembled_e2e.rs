//! CR14 end to end: a `POST /run` with a provenance dir appends one
//! `ContextAssembled` entry per model call (before the completion), and the
//! chain on disk still verifies. Needs the agent to send receipts (CR1) too.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_provenance::{ProvenanceEvent, ProvenanceLog};
use hone_providers::{MockProvider, ModelProvider};
use hone_server::{AppState, router};
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn test_cr14_run_should_append_a_context_assembled_entry_per_model_call() {
    let tmp = TempDir::new().unwrap();
    let provider: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["hello".into(), " world".into()]));
    let app = router(
        AppState::new(None)
            .with_mock_provider(provider)
            .with_provenance_dir(tmp.path().to_path_buf()),
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/run")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "prompt": "do a thing", "request_id": "ctx-session-1" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();

    let log = ProvenanceLog::open(&tmp.path().join("ctx-session-1.jsonl"))
        .expect("the chain reopens and verifies with the new entries");
    let ctx: Vec<usize> = log
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.event, ProvenanceEvent::ContextAssembled { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ctx.len(), 1, "one model call, one receipt entry");
    let completion = log
        .entries()
        .iter()
        .position(|e| matches!(e.event, ProvenanceEvent::ModelCompletion { .. }))
        .expect("the completion is still recorded");
    assert!(
        ctx[0] < completion,
        "the receipt precedes the completion it produced"
    );
    match &log.entries()[ctx[0]].event {
        ProvenanceEvent::ContextAssembled {
            receipt_sha256,
            strategies,
            ..
        } => {
            assert_eq!(receipt_sha256.len(), 64);
            assert!(!strategies.is_empty());
        }
        _ => unreachable!(),
    }
}
