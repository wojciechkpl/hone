//! Integration tests for `GET /files` and `GET /files/content`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tempfile::TempDir;
use tower::ServiceExt;

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("body is valid JSON")
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

fn state_with_workspace(tmp: &TempDir) -> AppState {
    AppState::new(None).with_workspace_root(tmp.path().to_path_buf())
}

#[tokio::test]
async fn list_files_returns_root_when_path_is_empty() {
    let tmp = TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("a.txt"), "alpha").expect("write");
    std::fs::create_dir(tmp.path().join("sub")).expect("mkdir");

    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(Request::builder().uri("/files").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let arr = body_json(resp).await;
    let list = arr.as_array().expect("array");
    // Two entries: sub/ (dir, sorted first) and a.txt (file).
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].get("name").and_then(|v| v.as_str()), Some("sub"));
    assert_eq!(list[0].get("is_dir").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(list[1].get("name").and_then(|v| v.as_str()), Some("a.txt"));
}

#[tokio::test]
async fn list_files_skips_hidden_entries() {
    let tmp = TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join(".hidden"), "x").expect("write");
    std::fs::write(tmp.path().join("visible.txt"), "y").expect("write");

    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(Request::builder().uri("/files").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let arr = body_json(resp).await;
    let names: Vec<&str> = arr
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"visible.txt"));
    assert!(!names.contains(&".hidden"));
}

#[tokio::test]
async fn list_files_returns_400_for_path_outside_workspace() {
    let tmp = TempDir::new().expect("tmp");
    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/files?path=../etc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn read_file_returns_text_content() {
    let tmp = TempDir::new().expect("tmp");
    std::fs::write(tmp.path().join("hello.txt"), "hi there").expect("write");
    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/files/content?path=hello.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.starts_with("text/plain"), "unexpected ct: {ct}");
    assert_eq!(body_text(resp).await, "hi there");
}

#[tokio::test]
async fn read_file_rejects_binary_files() {
    let tmp = TempDir::new().expect("tmp");
    // Write a small blob with a NUL byte.
    std::fs::write(tmp.path().join("blob.bin"), [1u8, 0, 2, 3]).expect("write");
    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/files/content?path=blob.bin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn read_file_returns_400_when_path_missing() {
    let tmp = TempDir::new().expect("tmp");
    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/files/content")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn read_file_returns_400_for_directory() {
    let tmp = TempDir::new().expect("tmp");
    std::fs::create_dir(tmp.path().join("sub")).expect("mkdir");
    let app = router(state_with_workspace(&tmp));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/files/content?path=sub")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
