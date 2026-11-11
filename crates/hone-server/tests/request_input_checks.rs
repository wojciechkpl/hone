#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W47 of `docs/plans/worker-tools-spec.md`: server input checks.
//! A `request_id` on `/run` or `/orchestrate` must be 1-128 characters of
//! `A-Z a-z 0-9 - _` (400 otherwise), because `/run` names its provenance file
//! after it and `../` escaped the provenance directory. `max_workers` must be
//! at least 1 (400 for 0, which panicked in `chunks(0)`).

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_providers::MockProvider;
use hone_server::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// `base/prov` is the provenance dir; anything created next to it (by a
/// `../` id) is an escape. Returns (state, base dir guard).
fn state_with_provenance() -> (AppState, tempfile::TempDir) {
    let base = tempfile::tempdir().unwrap();
    let prov = base.path().join("prov");
    std::fs::create_dir(&prov).unwrap();
    let state = AppState::new(None)
        .with_workspace_root(std::fs::canonicalize(base.path()).unwrap())
        .with_provenance_dir(prov)
        .with_mock_provider(Arc::new(MockProvider::new(vec!["ok".to_owned()])));
    (state, base)
}

fn bad_ids() -> Vec<String> {
    vec![
        "../evil".to_owned(),
        "a/b".to_owned(),
        String::new(),
        "a".repeat(129),
        "has space".to_owned(),
        "dot.dot".to_owned(),
        "..".to_owned(),
    ]
}

fn assert_nothing_escaped(base: &std::path::Path) {
    let mut names: Vec<String> = std::fs::read_dir(base)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["prov".to_owned()],
        "nothing next to the provenance dir"
    );
    assert_eq!(
        std::fs::read_dir(base.join("prov")).unwrap().count(),
        0,
        "no provenance file for a rejected request"
    );
}

#[tokio::test]
async fn test_w47_run_with_a_bad_request_id_should_be_400_and_create_no_file() {
    for id in bad_ids() {
        let (state, base) = state_with_provenance();

        let resp = router(state)
            .oneshot(post("/run", json!({ "prompt": "hi", "request_id": id })))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "request_id {id:?}");
        assert_nothing_escaped(base.path());
    }
}

#[tokio::test]
async fn test_w47_orchestrate_with_a_bad_request_id_should_be_400() {
    for id in bad_ids() {
        let (state, base) = state_with_provenance();

        let resp = router(state)
            .oneshot(post(
                "/orchestrate",
                json!({ "prompt": "hi", "request_id": id }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "request_id {id:?}");
        assert_nothing_escaped(base.path());
    }
}

#[tokio::test]
async fn test_w47_a_bad_request_id_in_step_mode_should_be_400_too() {
    let (state, _base) = state_with_provenance();

    let resp = router(state)
        .oneshot(post(
            "/orchestrate",
            json!({ "prompt": "hi", "request_id": "../evil",
                    "approval_policy": { "AskRisky": { "always_allowed": [] } } }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_w47_run_with_a_uuid_or_max_length_request_id_should_still_work() {
    for id in [
        uuid::Uuid::now_v7().to_string(),
        "A".repeat(128),
        "a-b_C-9".to_owned(),
    ] {
        let (state, base) = state_with_provenance();

        let resp = router(state)
            .oneshot(post("/run", json!({ "prompt": "hi", "request_id": id })))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK, "request_id {id:?}");
        let _ = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
        assert!(
            base.path()
                .join("prov")
                .join(format!("{id}.jsonl"))
                .exists(),
            "the provenance file is named after the id"
        );
    }
}

#[tokio::test]
async fn test_w47_orchestrate_with_max_workers_zero_should_be_400() {
    let (state, _base) = state_with_provenance();

    let resp = router(state)
        .oneshot(post(
            "/orchestrate",
            json!({ "prompt": "hi", "max_workers": 0 }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_w47_orchestrate_with_max_workers_one_should_still_work() {
    let (state, _base) = state_with_provenance();

    let resp = router(state)
        .oneshot(post(
            "/orchestrate",
            json!({ "prompt": "hi", "max_workers": 1 }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}
