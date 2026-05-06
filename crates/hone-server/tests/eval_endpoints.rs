//! Integration tests for `GET /evals` and `GET /evals/{id}`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_eval::{EvalRunStore, TaskOutcome, TaskStatus};
use hone_server::{AppState, router};
use tempfile::TempDir;
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

async fn open_store() -> (Arc<EvalRunStore>, TempDir) {
    let tmp = TempDir::new().expect("tmp");
    let path = tmp.path().join("eval.db");
    let store = EvalRunStore::open(&path).await.expect("open");
    (Arc::new(store), tmp)
}

fn fake_outcomes(passed: usize, total: usize) -> Vec<TaskOutcome> {
    (0..total)
        .map(|i| TaskOutcome {
            id: format!("t{i}"),
            status: if i < passed {
                TaskStatus::Pass
            } else {
                TaskStatus::Fail
            },
            duration_ms: 50,
            tokens_input: 1,
            tokens_output: 2,
            cost_usd: 0.01,
            success_check_stdout: String::new(),
            success_check_stderr: String::new(),
            success_check_exit_code: Some(0),
        })
        .collect()
}

#[tokio::test]
async fn list_endpoint_returns_503_when_no_store() {
    let state = AppState::new(None);
    let app = router(state);
    let resp = app
        .oneshot(Request::builder().uri("/evals").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn list_endpoint_returns_summaries_when_store_attached() {
    let (store, _tmp) = open_store().await;
    let r = store
        .record(None, "claude".into(), "h".into(), None, fake_outcomes(2, 3))
        .await
        .expect("record");

    let state = AppState::new(None).with_eval_history(store);
    let app = router(state);
    let resp = app
        .oneshot(Request::builder().uri("/evals").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let list = arr.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get("id").and_then(|v| v.as_str()), Some(r.id.as_str()));
    assert_eq!(list[0].get("passed").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(list[0].get("total").and_then(|v| v.as_u64()), Some(3));
    assert_eq!(list[0].get("model").and_then(|v| v.as_str()), Some("claude"));
}

#[tokio::test]
async fn list_endpoint_filters_by_tag() {
    let (store, _tmp) = open_store().await;
    let _r1 = store
        .record(None, "m1".into(), "h".into(), Some("baseline".into()), fake_outcomes(1, 1))
        .await
        .expect("r1");
    let _r2 = store
        .record(None, "m2".into(), "h".into(), Some("experiment".into()), fake_outcomes(1, 1))
        .await
        .expect("r2");

    let state = AppState::new(None).with_eval_history(store);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/evals?tag=baseline")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let arr = body_json(resp).await;
    let list = arr.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].get("tag").and_then(|v| v.as_str()), Some("baseline"));
}

#[tokio::test]
async fn detail_endpoint_returns_full_record_with_outcomes() {
    let (store, _tmp) = open_store().await;
    let r = store
        .record(None, "claude".into(), "h".into(), None, fake_outcomes(1, 2))
        .await
        .expect("record");

    let state = AppState::new(None).with_eval_history(store);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/evals/{}", r.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body.get("id").and_then(|v| v.as_str()), Some(r.id.as_str()));
    let outcomes = body.get("outcomes").and_then(|v| v.as_array()).expect("outcomes");
    assert_eq!(outcomes.len(), 2);
    let first = &outcomes[0];
    assert!(first.get("status").is_some());
    assert!(first.get("duration_ms").is_some());
    assert!(first.get("cost_usd").is_some());
}

#[tokio::test]
async fn detail_endpoint_returns_404_for_unknown_id() {
    let (store, _tmp) = open_store().await;
    let state = AppState::new(None).with_eval_history(store);
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/evals/no-such-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
