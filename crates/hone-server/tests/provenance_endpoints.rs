//! Integration tests for `GET /provenance`, `GET /provenance/{id}`,
//! `GET /provenance/{id}/verify`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_provenance::{ProvenanceEvent, ProvenanceLog};
use hone_server::{AppState, router};
use tempfile::TempDir;
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

fn write_chain(dir: &Path, name: &str, events: &[ProvenanceEvent]) {
    let path = dir.join(format!("{name}.jsonl"));
    let mut log = ProvenanceLog::open(&path).expect("open new");
    for ev in events {
        log.append(ev.clone()).expect("append");
    }
}

#[tokio::test]
async fn list_endpoint_returns_503_when_no_provenance_dir() {
    let state = AppState::new(None);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_endpoint_returns_summaries_when_dir_attached() {
    let tmp = TempDir::new().expect("tmp");
    write_chain(
        tmp.path(),
        "demo",
        &[
            ProvenanceEvent::UserPrompt { text: "hi".into() },
            ProvenanceEvent::SessionEnd { reason: "done".into() },
        ],
    );

    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].get("id").and_then(|v| v.as_str()), Some("demo"));
}

#[tokio::test]
async fn entries_endpoint_returns_full_chain() {
    let tmp = TempDir::new().expect("tmp");
    write_chain(
        tmp.path(),
        "session1",
        &[
            ProvenanceEvent::UserPrompt { text: "first".into() },
            ProvenanceEvent::ToolCall {
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp/x"}),
            },
        ],
    );

    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance/session1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0].get("index").and_then(|v| v.as_u64()), Some(0));
    assert_eq!(arr[1].get("index").and_then(|v| v.as_u64()), Some(1));
}

#[tokio::test]
async fn entries_endpoint_returns_404_for_unknown_id() {
    let tmp = TempDir::new().expect("tmp");
    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance/no-such-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn verify_endpoint_returns_valid_for_clean_chain() {
    let tmp = TempDir::new().expect("tmp");
    write_chain(
        tmp.path(),
        "ok",
        &[
            ProvenanceEvent::UserPrompt { text: "a".into() },
            ProvenanceEvent::SessionEnd { reason: "done".into() },
        ],
    );

    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance/ok/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body.get("valid").and_then(|v| v.as_bool()), Some(true));
    assert!(body.get("merkle_root").and_then(|v| v.as_str()).is_some());
    assert_eq!(body.get("entry_count").and_then(|v| v.as_u64()), Some(2));
}

#[tokio::test]
async fn verify_endpoint_pinpoints_tampering() {
    let tmp = TempDir::new().expect("tmp");
    write_chain(
        tmp.path(),
        "tampered",
        &[
            ProvenanceEvent::UserPrompt { text: "a".into() },
            ProvenanceEvent::UserPrompt { text: "b".into() },
        ],
    );
    // Tamper with the second entry's prev_hash.
    let path = tmp.path().join("tampered.jsonl");
    let body = std::fs::read_to_string(&path).expect("read");
    let mut lines: Vec<String> = body.lines().map(String::from).collect();
    let mut second: hone_provenance::ProvenanceEntry =
        serde_json::from_str(&lines[1]).expect("parse");
    second.prev_hash = "f".repeat(64);
    lines[1] = serde_json::to_string(&second).expect("serialise");
    std::fs::write(&path, lines.join("\n")).expect("write");

    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance/tampered/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body.get("valid").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(body.get("broken_at").and_then(|v| v.as_u64()), Some(1));
}

#[tokio::test]
async fn entries_endpoint_rejects_path_traversal() {
    let tmp = TempDir::new().expect("tmp");
    let state = AppState::new(None).with_provenance_dir(tmp.path().to_path_buf());
    let app = router(state);
    // axum's Path matcher refuses many forms, but `..` may sneak through;
    // the handler defends in depth.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/provenance/..%2Fetc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(resp.status(), StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND),
        "unexpected status: {}",
        resp.status(),
    );
}
